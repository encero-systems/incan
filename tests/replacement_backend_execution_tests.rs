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
use incan_semantics_core::body_ir::{
    BodyIrModule, ConstructorTarget, OwnershipFact, Rvalue, StatementKind, TryErrorRouting,
};
use incan_semantics_core::{CompilerNodeId, IncanType};

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

/// Lower one named module against a shared checker so two modules can be produced for cross-module tests.
///
/// `lower_typed_body_ir` fixes the module path, which is right for the single-module #988 profile but cannot express
/// a call that crosses a module edge. #1260 needs two Body-IR modules whose canonical identities were minted under
/// their own module paths, which is what this produces.
fn lower_named_body_ir(source: &str, module_path: &[&str]) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|part| (*part).to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Characterize the module boundary #1260 replaces: a cross-module call cannot resolve through one module.
///
/// `prepare_free_function_execution` and `execute_free_function` take one `&BodyIrModule`, so a body whose reachable
/// call graph leaves that module has no way to reach its callee: an imported callable deliberately carries no
/// same-module `direct_call_id`, and #1260 must resolve it from the compiler-owned canonical target rather than
/// manufacture one. This pins the current boundary so the `ReplacementExecutionGraph` that replaces it has a failing
/// test to satisfy, and so the refusal stays a refusal rather than silently executing the wrong body.
///
/// This passes today, so it is a characterization test rather than a failing one. Its value is the invariant it
/// pins: a cross-module call must never resolve against the caller's own module. When #1260 makes such a call
/// execute, this test inverts to assert the value it produces, and until then it stops a partial implementation from
/// silently resolving the wrong body.
#[test]
fn a_cross_module_call_is_refused_by_the_single_module_executor() -> Result<(), Box<dyn std::error::Error>> {
    let provider = lower_named_body_ir("pub def provide() -> int:\n  return 7\n", &["cross_module_provider"])?;
    let consumer = lower_named_body_ir(
        "from cross_module_provider import provide\n\ndef main() -> int:\n  return provide()\n",
        &["cross_module_consumer"],
    )?;

    // The provider's body exists, but it exists in the *other* module.
    assert!(
        provider.bodies.iter().any(|body| body.name == "provide"),
        "the provider module must retain its own callable body"
    );
    assert!(
        !consumer.bodies.iter().any(|body| body.name == "provide"),
        "the consumer module must not contain the imported callee's body; a cross-module call has to be resolved \
         through the module graph rather than found locally"
    );

    let executed = execute_free_function(&consumer, "main", &[]);
    assert!(
        executed.is_err(),
        "executing a cross-module call against only the caller's module must refuse until #1260 supplies the \
         execution graph, got: {executed:?}"
    );
    Ok(())
}

/// Legal source used to isolate malformed retained constructor facts after checked lowering.
const PAIR_CONSTRUCTION_SOURCE: &str = r#"
model Pair:
  left: int
  right: int

def main() -> int:
  pair = Pair(left=40, right=2)
  return pair.left
"#;

/// Locate the one Pair construction whose post-lowering facts each malformed-IR test changes deliberately.
fn pair_constructor_target_mut(
    module: &mut BodyIrModule,
) -> Result<&mut ConstructorTarget, Box<dyn std::error::Error>> {
    let main = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("fixture must lower the main Body-IR body")?;
    main.block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Assign {
                rvalue: Rvalue::Aggregate(incan_semantics_core::body_ir::AggregateKind::Constructor(target), _),
                ..
            } => Some(target.as_mut()),
            _ => None,
        })
        .ok_or("fixture must lower its model construction as a constructor aggregate".into())
}

/// Require malformed retained Pair facts to refuse before direct execution can yield a result or receipt input.
///
/// The public command cannot accept malformed internal Body IR, so this direct-executor seam is the honest proof:
/// an `Err` has no `ReplacementExecution` result that a command could bind into a successful receipt.
fn assert_malformed_pair_constructor_refusal(
    module: &BodyIrModule,
    expected_description: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let error = match execute_free_function(module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "malformed retained Pair facts must refuse instead of executing, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let constructor_start = PAIR_CONSTRUCTION_SOURCE
        .find("Pair(left=40, right=2)")
        .ok_or("fixture must contain the model constructor")?;
    let span = error
        .primary_span()
        .ok_or("malformed retained Pair facts must retain a source span")?;
    assert_eq!(span.start, constructor_start);
    assert_eq!(span.end, constructor_start + "Pair(left=40, right=2)".len());
    assert!(
        error.to_string().contains(expected_description),
        "the refusal must name the malformed retained fact: {error}"
    );
    Ok(())
}

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

/// Execute a fully supplied source-local model through direct storage, canonical field reads, and sibling dispatch.
#[test]
fn replacement_executes_source_local_nominal_model_values_through_a_direct_callable()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Pair:
  left: int
  right: int = 2

def score(pair: Pair) -> int:
  return pair.left + pair.right

def main() -> int:
  pair = Pair(right=2, left=40)
  return score(pair)
"#;
    let module = lower_typed_body_ir(source)?;
    let declaration = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == "Pair")
        .ok_or("fixture must retain the source-local Pair declaration")?;
    let constructor_evidence = format!(
        "executed nominal constructor name=Pair id={} fields=[left, right]",
        declaration.direct_declaration_id
    );
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution.body_snapshot.contains("body score"),
        "the nominal value must cross an exact same-module direct call: {}",
        execution.body_snapshot
    );
    assert!(
        execution.body_snapshot.contains(&constructor_evidence),
        "the direct evidence must bind Pair to its retained declaration identity and canonical layout: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Execute source-local nominal and normal-enum pattern dispatch without recovering declarations from spellings.
#[test]
fn replacement_executes_identity_selected_nominal_and_fieldless_enum_match_patterns()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Pair:
  left: int
  right: int

enum Signal:
  Ready
  Stop

def classify(pair: Pair, signal: Signal) -> int:
  match pair:
    case Pair(left=40, right=2):
      match signal:
        case Signal.Ready:
          return 42
        case Signal.Stop:
          return 0
    case _:
      return 0
  return 0

def main() -> int:
    return classify(Pair(left=40, right=2), Signal.Ready)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution.body_snapshot.contains("executed direct match arm"),
        "direct execution must retain selected pattern-arm evidence: {}",
        execution.body_snapshot
    );
    Ok(())
}

#[test]
fn replacement_refuses_a_nominal_pattern_after_its_exact_target_identity_is_removed()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Point:
  x: int

def main() -> int:
  point = Point(x=42)
  match point:
    case Point(x=value):
      return value
  return 0
"#;
    let mut module = lower_typed_body_ir(source)?;
    let pattern = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .and_then(|body| {
            body.block
                .stmts
                .iter_mut()
                .find_map(|statement| match &mut statement.kind {
                    StatementKind::Assign {
                        rvalue: Rvalue::Match { arms, .. },
                        ..
                    } => arms.first_mut().map(|arm| &mut arm.pattern),
                    _ => None,
                })
        })
        .ok_or("fixture must lower a nominal pattern")?;
    let fields = match pattern {
        incan_semantics_core::body_ir::Pattern::Nominal { fields, .. } => fields.clone(),
        other => return Err(format!("fixture must retain the nominal identity: {other:?}").into()),
    };
    *pattern = incan_semantics_core::body_ir::Pattern::Struct {
        canonical: None,
        name: "Point".to_string(),
        fields,
    };

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(_) => return Err("a pattern without its exact canonical target must refuse".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("match pattern without an exact direct target identity")
    );
    Ok(())
}

/// Execute intrinsic Result construction, same-error `?` routing, and the checked `Ok`/`Err` pattern facts.
#[test]
fn replacement_executes_same_error_result_routing_and_pattern_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Failure:
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

def failed() -> int:
  match quarter(5):
    case Ok(value):
      return value
    case Err(_):
      return 0
  return 0

def pair_result() -> Result[tuple[int, int], Failure]:
  return Ok((20, 22))

def tuple_payload() -> int:
  match pair_result():
    case Ok(_):
      return 42
    case Err(_):
      return 0
  return 0
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(2));
    assert!(
        execution.body_snapshot.contains("executed Result::ok construction")
            && execution.body_snapshot.contains("executed Result try route=ok"),
        "receipt-bound evidence must name direct Result construction and same-error routing: {}",
        execution.body_snapshot
    );
    let propagated = execute_free_function(&module, "failed", &[])?;
    assert!(
        propagated.value == ReplacementValue::Int(0)
            && propagated.body_snapshot.contains("executed Result try route=err"),
        "an Err must return from `?` directly into the enclosing Result caller: {}",
        propagated.body_snapshot
    );
    let tuple_payload = execute_free_function(&module, "tuple_payload", &[])?;
    assert_eq!(
        tuple_payload.value,
        ReplacementValue::Int(42),
        "a recursively structural tuple Result payload must retain its checked direct type"
    );

    let mut conversion_required = module.clone();
    let mut try_span = None;
    for statement in &mut conversion_required
        .bodies
        .iter_mut()
        .find(|body| body.name == "quarter")
        .ok_or("fixture must retain the quarter body")?
        .block
        .stmts
    {
        if let StatementKind::TryPropagate { error_routing, .. } = &mut statement.kind {
            try_span = Some(statement.span);
            *error_routing = TryErrorRouting::ConversionRequired {
                source_error_type: IncanType::Named("Failure".to_string()),
                destination_error_type: IncanType::Named("OtherFailure".to_string()),
            };
        }
    }
    let try_span = try_span.ok_or("fixture must lower a try-propagate statement")?;
    let error = match execute_free_function(&conversion_required, "failed", &[]) {
        Ok(execution) => {
            return Err(format!(
                "cross-error Result propagation must refuse directly, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            incan::backend::replacement::ReplacementExecutionError::Unsupported {
                ref description,
                span,
                ..
            } if description == "cross-error-type try propagation" && span == try_span
        ),
        "conversion-required Result routing must refuse at its original `?` span: {error:?}"
    );

    let mut mismatched_payload = module.clone();
    let mut result_span = None;
    for statement in &mut mismatched_payload
        .bodies
        .iter_mut()
        .find(|body| body.name == "half")
        .ok_or("fixture must retain the half body")?
        .block
        .stmts
    {
        let StatementKind::Assign {
            rvalue: Rvalue::ResultVariant(variant),
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        if variant.kind == incan_semantics_core::body_ir::ResultVariantKind::Ok {
            result_span = Some(statement.span);
            variant.ok_type = IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str);
        }
    }
    let result_span = result_span.ok_or("fixture must lower an Ok Result construction")?;
    let error = match execute_free_function(&mismatched_payload, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a Result payload that disagrees with its retained checked type must refuse, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            incan::backend::replacement::ReplacementExecutionError::Unsupported {
                ref description,
                span,
                ..
            } if description.contains("payload incompatible with retained type `str`") && span == result_span
        ),
        "a malformed Result payload type must refuse at its original constructor span: {error:?}"
    );
    Ok(())
}

/// Execute exact source-local RFC 032 value-enum members through direct sibling dispatch and scalar extraction.
#[test]
fn replacement_executes_source_local_value_enum_members_through_a_direct_callable()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum HttpStatus(int):
  Ok = 200
  NotFound = 404

enum Environment(str):
  Development = "development"
  Production = "production"

def status_code(status: HttpStatus) -> int:
  return status.value()

def extract_environment(environment: Environment) -> str:
  return environment.value()

def environment_name() -> str:
  return extract_environment(Environment.Production)

def main() -> int:
  return status_code(HttpStatus.NotFound)
"#;
    let module = lower_typed_body_ir(source)?;
    let status_declaration = module
        .value_enum_declarations
        .iter()
        .find(|declaration| declaration.name == "HttpStatus")
        .ok_or("fixture must retain the source-local HttpStatus declaration")?;
    let not_found = status_declaration
        .variants
        .iter()
        .find(|variant| variant.name == "NotFound")
        .ok_or("fixture must retain the source-local NotFound member")?;
    let execution = execute_free_function(&module, "main", &[])?;
    let environment_execution = execute_free_function(&module, "environment_name", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(404));
    assert_eq!(
        environment_execution.value,
        ReplacementValue::Str("production".to_string())
    );
    assert!(
        execution.body_snapshot.contains("body status_code"),
        "the integer scalar extraction must execute through an exact same-module body: {}",
        execution.body_snapshot
    );
    assert!(
        execution.body_snapshot.contains(&format!(
            "executed value-enum variant name=HttpStatus::NotFound enum_id={} variant_id={} raw=404",
            status_declaration.direct_declaration_id, not_found.direct_declaration_id
        )),
        "the direct evidence must bind the executed member to exact retained identities: {}",
        execution.body_snapshot
    );
    assert!(
        environment_execution.body_snapshot.contains("body extract_environment")
            && environment_execution
                .body_snapshot
                .contains("extracted value-enum scalar name=Environment::Production"),
        "the generated `.value()` surface must extract through an identity-validated runtime carrier: {}",
        environment_execution.body_snapshot
    );
    Ok(())
}

/// Execute source-local fieldless normal-enum values only through exact identities and scalar comparison.
#[test]
fn replacement_executes_source_local_fieldless_enum_values_through_a_direct_callable()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
  Ready
  Stop

def score(left: Signal, right: Signal) -> int:
  if left == Signal.Ready and right != Signal.Ready:
    return 42
  return 0

def normal_enum_values() -> int:
  return score(Signal.Ready, Signal.Stop)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "normal_enum_values", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution
            .body_snapshot
            .contains("executed fieldless-enum variant name=Signal::Ready"),
        "the direct execution evidence must name the first retained normal-enum member: {}",
        execution.body_snapshot
    );
    assert!(
        execution
            .body_snapshot
            .contains("executed fieldless-enum variant name=Signal::Stop"),
        "the direct execution evidence must name the second retained normal-enum member: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Refuse a removed fieldless-enum registry rather than recovering a member from its source spelling.
#[test]
fn replacement_refuses_a_fieldless_enum_member_without_its_retained_identity_at_its_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
  Ready
  Stop

def main() -> bool:
  return Signal.Ready == Signal.Stop
"#;
    let mut module = lower_typed_body_ir(source)?;
    module.fieldless_enum_declarations.clear();

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a member without its retained identity must refuse instead of dispatching by name, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let member_start = source
        .find("Signal.Ready")
        .ok_or("fixture must contain the rejected member expression")?;
    let span = error
        .primary_span()
        .ok_or("fieldless-enum identity refusal must retain a source span")?;
    assert_eq!(span.start, member_start);
    assert_eq!(span.end, member_start + "Signal.Ready".len());
    assert!(
        error
            .to_string()
            .contains("fieldless-enum member targets a declaration outside this Body-IR module"),
        "the refusal must name the missing retained identity: {error}"
    );
    Ok(())
}

/// Refuse a coherent-looking fieldless-enum registry whose identities name another Body-IR module.
#[test]
fn replacement_refuses_a_foreign_fieldless_enum_identity_at_its_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
  Ready
  Stop

def main() -> bool:
  return Signal.Ready == Signal.Stop
"#;
    let mut module = lower_typed_body_ir(source)?;
    let foreign_enum_id = CompilerNodeId::declaration_span("foreign", 0, 32);
    let foreign_variant_id = CompilerNodeId::declaration_span("foreign", 14, 19);
    {
        let declaration = module
            .fieldless_enum_declarations
            .first_mut()
            .ok_or("fixture must retain one source-local fieldless enum")?;
        declaration.direct_declaration_id = foreign_enum_id.clone();
        let variant = declaration
            .variants
            .iter_mut()
            .find(|variant| variant.name == "Ready")
            .ok_or("fixture must retain the selected fieldless-enum member")?;
        variant.direct_declaration_id = foreign_variant_id.clone();
    }
    let target = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .and_then(|body| {
            body.block
                .stmts
                .iter_mut()
                .find_map(|statement| match &mut statement.kind {
                    StatementKind::Assign {
                        rvalue: Rvalue::FieldlessEnumVariant(target),
                        ..
                    } => Some(target),
                    _ => None,
                })
        })
        .ok_or("fixture must lower the selected member as a fieldless-enum rvalue")?;
    target.enum_declaration_id = foreign_enum_id;
    target.variant_declaration_id = foreign_variant_id;

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a foreign fieldless-enum identity must refuse instead of executing, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let member_start = source
        .find("Signal.Ready")
        .ok_or("fixture must contain the rejected member expression")?;
    let span = error
        .primary_span()
        .ok_or("foreign fieldless-enum identity refusal must retain a source span")?;
    assert_eq!(span.start, member_start);
    assert_eq!(span.end, member_start + "Signal.Ready".len());
    assert!(
        error
            .to_string()
            .contains("fieldless-enum member declaration identity is not scoped to this Body-IR module"),
        "the refusal must name the foreign declaration identity: {error}"
    );
    Ok(())
}

/// Refuse a removed source-local value-enum registry rather than recovering a member from its spelling.
#[test]
fn replacement_refuses_a_value_enum_member_without_its_retained_identity_at_its_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def main() -> int:
  return HttpStatus.NotFound.value()
"#;
    let mut module = lower_typed_body_ir(source)?;
    module.value_enum_declarations.clear();

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a member without its retained identity must refuse instead of dispatching by name, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let member_start = source
        .find("HttpStatus.NotFound")
        .ok_or("fixture must contain the rejected member expression")?;
    let span = error
        .primary_span()
        .ok_or("value-enum identity refusal must retain a source span")?;
    assert_eq!(span.start, member_start);
    assert_eq!(span.end, member_start + "HttpStatus.NotFound".len());
    assert!(
        error
            .to_string()
            .contains("value-enum member targets a declaration outside this Body-IR module"),
        "the refusal must name the missing retained identity: {error}"
    );
    Ok(())
}

/// Refuse a coherent-looking value-enum registry whose identities name another Body-IR module.
#[test]
fn replacement_refuses_a_foreign_value_enum_identity_at_its_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def main() -> int:
  return HttpStatus.NotFound.value()
"#;
    let mut module = lower_typed_body_ir(source)?;
    let foreign_enum_id = CompilerNodeId::declaration_span("foreign", 0, 53);
    let foreign_variant_id = CompilerNodeId::declaration_span("foreign", 21, 35);
    {
        let declaration = module
            .value_enum_declarations
            .first_mut()
            .ok_or("fixture must retain one source-local value enum")?;
        declaration.direct_declaration_id = foreign_enum_id.clone();
        let variant = declaration
            .variants
            .iter_mut()
            .find(|variant| variant.name == "NotFound")
            .ok_or("fixture must retain the selected value-enum member")?;
        variant.direct_declaration_id = foreign_variant_id.clone();
    }
    let target = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .and_then(|body| {
            body.block
                .stmts
                .iter_mut()
                .find_map(|statement| match &mut statement.kind {
                    StatementKind::Assign {
                        rvalue: Rvalue::ValueEnumVariant(target),
                        ..
                    } => Some(target),
                    _ => None,
                })
        })
        .ok_or("fixture must lower the selected member as a value-enum rvalue")?;
    target.enum_declaration_id = foreign_enum_id;
    target.variant_declaration_id = foreign_variant_id;

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a foreign value-enum identity must refuse instead of executing, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let member_start = source
        .find("HttpStatus.NotFound")
        .ok_or("fixture must contain the rejected member expression")?;
    let span = error
        .primary_span()
        .ok_or("foreign value-enum identity refusal must retain a source span")?;
    assert_eq!(span.start, member_start);
    assert_eq!(span.end, member_start + "HttpStatus.NotFound".len());
    assert!(
        error
            .to_string()
            .contains("value-enum member declaration identity is not scoped to this Body-IR module"),
        "the refusal must name the foreign declaration identity: {error}"
    );
    Ok(())
}

/// Refuse a constructor when its retained declaration layout no longer matches checked constructor slots.
///
/// This injects malformed Body IR after lowering; valid source cannot reorder the retained declaration. The direct
/// executor must fail at the original construction rather than return a value with fields shifted by the mutation.
#[test]
fn replacement_refuses_a_reordered_nominal_declaration_layout_at_the_original_constructor_span()
-> Result<(), Box<dyn std::error::Error>> {
    let mut module = lower_typed_body_ir(PAIR_CONSTRUCTION_SOURCE)?;
    let declaration = module
        .nominal_declarations
        .iter_mut()
        .find(|declaration| declaration.name == "Pair")
        .ok_or("fixture must retain the source-local Pair declaration")?;
    let [left, right] = declaration.fields.as_mut_slice() else {
        return Err("fixture must retain Pair's two canonical fields".into());
    };
    std::mem::swap(left, right);

    assert_malformed_pair_constructor_refusal(
        &module,
        "constructor canonical target disagrees with the retained declaration identity",
    )
}

/// Refuse a constructor missing the checked layout that pairs its numeric slots with retained field names.
#[test]
fn replacement_refuses_a_nominal_constructor_missing_checked_field_layout_at_the_original_span()
-> Result<(), Box<dyn std::error::Error>> {
    let mut module = lower_typed_body_ir(PAIR_CONSTRUCTION_SOURCE)?;
    pair_constructor_target_mut(&mut module)?.canonical_field_layout = None;

    assert_malformed_pair_constructor_refusal(&module, "constructor `Pair` without a checked canonical field layout")
}

/// Refuse a constructor whose independently retained layout omits one checked field slot.
#[test]
fn replacement_refuses_a_nominal_constructor_with_a_short_checked_field_layout_at_the_original_span()
-> Result<(), Box<dyn std::error::Error>> {
    let mut module = lower_typed_body_ir(PAIR_CONSTRUCTION_SOURCE)?;
    let layout = pair_constructor_target_mut(&mut module)?
        .canonical_field_layout
        .as_mut()
        .ok_or("fixture must retain Pair's checked canonical field layout")?;
    let removed = layout
        .pop()
        .ok_or("fixture must retain the right field in Pair's checked layout")?;
    assert_eq!(removed, "right");

    assert_malformed_pair_constructor_refusal(
        &module,
        "canonical field layout disagrees with checked constructor facts",
    )
}

/// Refuse a constructor whose independently retained layout changes one canonical field name.
#[test]
fn replacement_refuses_a_nominal_constructor_with_a_renamed_checked_field_at_the_original_span()
-> Result<(), Box<dyn std::error::Error>> {
    let mut module = lower_typed_body_ir(PAIR_CONSTRUCTION_SOURCE)?;
    let layout = pair_constructor_target_mut(&mut module)?
        .canonical_field_layout
        .as_mut()
        .ok_or("fixture must retain Pair's checked canonical field layout")?;
    let first = layout
        .first_mut()
        .ok_or("fixture must retain the left field in Pair's checked layout")?;
    *first = "renamed".to_string();

    assert_malformed_pair_constructor_refusal(
        &module,
        "canonical field layout disagrees with checked constructor facts",
    )
}

/// Refuse a constructor whose retained declaration identity points outside the current Body-IR module.
#[test]
fn replacement_refuses_a_foreign_nominal_constructor_identity_at_the_original_span()
-> Result<(), Box<dyn std::error::Error>> {
    let mut module = lower_typed_body_ir(PAIR_CONSTRUCTION_SOURCE)?;
    pair_constructor_target_mut(&mut module)?.direct_declaration_id =
        Some(CompilerNodeId::declaration_span("foreign", 0, 12));

    assert_malformed_pair_constructor_refusal(
        &module,
        "constructor canonical target disagrees with its physical Body-IR declaration",
    )
}

#[test]
fn replacement_refuses_enum_targets_whose_canonical_member_disagrees_with_the_physical_record()
-> Result<(), Box<dyn std::error::Error>> {
    let fieldless_source =
        "enum Signal:\n  Ready\n  Stop\n\ndef main() -> bool:\n  return Signal.Ready == Signal.Stop\n";
    let mut fieldless = lower_typed_body_ir(fieldless_source)?;
    let fieldless_target = fieldless
        .bodies
        .iter_mut()
        .flat_map(|body| &mut body.block.stmts)
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Assign {
                rvalue: Rvalue::FieldlessEnumVariant(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("fixture must lower a fieldless-enum member")?;
    fieldless_target.variant_canonical.declaration_name = "Stop".to_string();
    let fieldless_error = match execute_free_function(&fieldless, "main", &[]) {
        Ok(_) => return Err("a mismatched fieldless-enum canonical target must refuse".into()),
        Err(error) => error,
    };
    assert!(
        fieldless_error
            .to_string()
            .contains("without exact canonical source-local owner/member identities")
    );

    let value_source =
        "enum Status(int):\n  Ok = 200\n  Missing = 404\n\ndef main() -> int:\n  return Status.Missing.value()\n";
    let mut value = lower_typed_body_ir(value_source)?;
    let value_target = value
        .bodies
        .iter_mut()
        .flat_map(|body| &mut body.block.stmts)
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Assign {
                rvalue: Rvalue::ValueEnumVariant(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("fixture must lower a value-enum member")?;
    value_target.enum_canonical.declaration_name = "Other".to_string();
    let value_error = match execute_free_function(&value, "main", &[]) {
        Ok(_) => return Err("a mismatched value-enum canonical target must refuse".into()),
        Err(error) => error,
    };
    assert!(
        value_error
            .to_string()
            .contains("without exact canonical source-local owner/member identities")
    );
    Ok(())
}

/// Refuse a constructor whose source-local declaration identity is absent instead of recovering it from its name.
#[test]
fn replacement_refuses_an_idless_nominal_constructor_at_its_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Pair:
  left: int
  right: int

def main() -> int:
  pair = Pair(right=2, left=40)
  return pair.left + pair.right
"#;
    let mut module = lower_typed_body_ir(source)?;
    let main = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("fixture must lower the main Body-IR body")?;
    let target = main
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            incan_semantics_core::body_ir::StatementKind::Assign {
                rvalue:
                    incan_semantics_core::body_ir::Rvalue::Aggregate(
                        incan_semantics_core::body_ir::AggregateKind::Constructor(target),
                        _,
                    ),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("fixture must lower its model construction as a constructor aggregate")?;
    assert!(
        target.direct_declaration_id.is_some(),
        "a source-local model construction must retain an explicit declaration identity"
    );
    target.direct_declaration_id = None;

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "an id-less nominal constructor must refuse instead of dispatching by name, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let constructor_start = source
        .find("Pair(right=2, left=40)")
        .ok_or("fixture must contain the model constructor")?;
    let span = error
        .primary_span()
        .ok_or("id-less nominal constructor refusal must retain a source span")?;
    assert_eq!(span.start, constructor_start);
    assert_eq!(span.end, constructor_start + "Pair(right=2, left=40)".len());
    assert!(
        error
            .to_string()
            .contains("constructor `Pair` without a source-local declaration identity"),
        "the refusal must name the missing nominal fact: {error}"
    );
    Ok(())
}

/// Refuse a nominal field default until Body IR retains its declaration-owned computation.
#[test]
fn replacement_refuses_an_omitted_nominal_field_default_at_the_constructor_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Pair:
  left: int
  right: int = 2

def main() -> int:
  pair = Pair(left=40)
  return pair.left
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a nominal construction with an omitted field default must refuse, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let constructor_start = source
        .find("Pair(left=40)")
        .ok_or("fixture must contain the defaulted model constructor")?;
    let span = error
        .primary_span()
        .ok_or("defaulted nominal constructor refusal must retain a source span")?;
    assert_eq!(span.start, constructor_start);
    assert_eq!(span.end, constructor_start + "Pair(left=40)".len());
    assert!(
        error
            .to_string()
            .contains("constructor `Pair` with an omitted field default"),
        "the refusal must name the unrepresented default: {error}"
    );
    Ok(())
}

/// Keep nominal field writes outside the read-only direct model profile.
#[test]
fn replacement_refuses_nominal_field_assignment_at_the_original_source_span() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
model Pair:
  left: int
  right: int

def main() -> int:
  pair = Pair(left=40, right=2)
  pair.left = 41
  return pair.left
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a nominal field assignment must refuse in the read-only profile, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let assignment_start = source
        .find("pair.left = 41")
        .ok_or("fixture must contain the nominal field assignment")?;
    let span = error
        .primary_span()
        .ok_or("nominal field assignment refusal must retain a source span")?;
    assert_eq!(span.start, assignment_start);
    assert_eq!(span.end, assignment_start + "pair.left = 41".len());
    assert!(
        error.to_string().contains("field assignment"),
        "the refusal must name the unavailable write: {error}"
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

/// An async declaration constructs a direct task even when its child body has no suspension of its own.
#[test]
fn replacement_executes_a_source_local_async_task_and_binds_its_lifecycle_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def child() -> int:
  return 42

async def main() -> int:
  return await child()
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    let lifecycle = execution.task_lifecycle_evidence();
    assert!(
        lifecycle.iter().filter(|event| event.event == "constructed").count() == 2,
        "the root and child must each construct a direct task frame: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().any(|event| event.event == "await_suspended")
            && lifecycle.iter().any(|event| event.event == "await_resumed"),
        "the parent task must retain an explicit await suspension and resume: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().filter(|event| event.event == "completed").count() == 2,
        "both task frames must complete through direct Body IR: {lifecycle:?}"
    );
    assert!(
        execution
            .runtime_requirement_evidence()
            .iter()
            .any(|requirement| requirement.requirement == "async_runtime"),
        "an async body without an explicit child suspension still needs direct async runtime evidence"
    );
    Ok(())
}

/// Constructing and dropping a same-module task records construction without polling the child's body.
#[test]
fn replacement_records_construct_only_task_lifecycle_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def child() -> int:
  assert false
  return 7

def main() -> int:
  child()
  return 1
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(1));
    let lifecycle = execution.task_lifecycle_evidence();
    assert_eq!(
        lifecycle.len(),
        1,
        "only the unpolled child task should be observed: {lifecycle:?}"
    );
    let event = lifecycle
        .first()
        .ok_or("construction-only task must record its construction")?;
    assert_eq!(event.event, "constructed");
    Ok(())
}

/// A ready race constructs every arm, then polls and selects the first source-order arm while cancelling later arms
/// without running their bodies.
#[test]
fn replacement_executes_source_order_async_race_ties_with_loser_cancellation() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
import std.async

async def first() -> int:
  return 1

async def second() -> int:
  return 2

async def main() -> int:
  winner = race for value:
    await first() => value
    await second() =>
      assert value == 999
      value
  return winner
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(1));
    let lifecycle = execution.task_lifecycle_evidence();
    let winner = lifecycle
        .iter()
        .position(|event| event.event == "race_winner")
        .ok_or("race must retain its winning task transition")?;
    let last_constructed = lifecycle
        .iter()
        .rposition(|event| event.event == "constructed")
        .ok_or("race must construct its arm tasks")?;
    assert!(
        last_constructed < winner,
        "every race arm must be constructed before ready-tie selection: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().any(|event| event.event == "cancelled"),
        "the non-winning constructed task must be cancelled at the race boundary: {lifecycle:?}"
    );
    Ok(())
}

/// Reversing simultaneously ready source arms reverses the selected result without polling the cancelled loser.
#[test]
fn replacement_race_ready_ties_follow_source_order_and_cancel_unpolled_losers() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
import std.async

async def first() -> int:
  return 1

async def second() -> int:
  return 2

async def main() -> int:
  winner = race for value:
    await second() => value
    await first() => value
  return winner
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(2));
    let lifecycle = execution.task_lifecycle_evidence();
    let first_winner = lifecycle
        .iter()
        .position(|event| event.event == "race_winner")
        .ok_or("race must retain its winning task transition")?;
    let polls_before_winner = lifecycle[..first_winner]
        .iter()
        .filter(|event| event.event == "polled")
        .count();
    assert_eq!(
        polls_before_winner, 2,
        "the root and source-order winner must poll before selection, while the loser stays unpolled: {lifecycle:?}"
    );
    assert!(
        lifecycle.iter().any(|event| event.event == "cancelled"),
        "the unpolled later arm must be cancelled at the winner boundary: {lifecycle:?}"
    );
    Ok(())
}

/// A failing first race arm retains its source failure while later constructed arms stay unpolled and are cancelled.
#[test]
fn replacement_race_failure_preserves_the_first_arm_span_and_cancels_constructed_losers()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def first() -> int:
  assert false
  return 1

async def loser() -> int:
  assert false
  return 2

async def main() -> int:
  winner = race for value:
    await first() => value
    await loser() => value
  return winner
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("failing first race arm executed as {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("assert false")
        .ok_or("fixture must contain the first arm assertion")?;
    let span = error
        .primary_span()
        .ok_or("race winner failure must retain its original source span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error.to_string().contains("assertion failed"),
        "the first task failure must remain the direct runtime failure: {error}"
    );
    Ok(())
}

/// A failure in an admitted child task stays a direct runtime failure at its original source statement.
#[test]
fn replacement_propagates_async_task_failure_at_the_child_statement_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async

async def child() -> int:
  assert false
  return 7

async def main() -> int:
  return await child()
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("failing async child executed as {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("assert false")
        .ok_or("fixture must contain the child assertion")?;
    let span = error
        .primary_span()
        .ok_or("async child failure must retain its original source span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error.to_string().contains("assertion failed"),
        "child task failure must stay a direct runtime failure: {error}"
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
        "replacement build must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "non-printing programs must leave stdout empty"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");
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
        "typed empty scalar-pair list must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "non-printing programs must leave stdout empty"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");
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
    fs::write(
        &entrypoint,
        "def main() -> int:\n  println(\"answer follows\")\n  return 42\n",
    )?;

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
        "replacement JSON report must succeed. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["schema_version"], "incan.replacement_execution.v1");
    assert_eq!(report["status"], "success");
    assert_eq!(report["mode"], "executable");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert_eq!(report["replacement_execution"]["result_type"], "int");
    assert_eq!(
        report["replacement_execution"]["emitted_output"],
        serde_json::json!(["answer follows"])
    );
    assert_eq!(output.stdout, b"answer follows\n");
    assert_eq!(
        report["replacement_execution"]["stdout_bytes"],
        serde_json::json!(output.stdout)
    );
    assert_eq!(report["replacement_execution"]["stderr_bytes"], serde_json::json!([]));
    assert!(
        output.stderr.is_empty(),
        "report metadata must not displace program output"
    );
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
        report["replacement_execution"]["task_lifecycle"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "a direct scalar execution that never constructs a task must publish an empty task lifecycle: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "direct replacement report must not invent generated-Rust or Oven evidence: {report}"
    );
    Ok(())
}

/// Execute only the session-projected `beta` entry and bind its semantic module to both report and receipt evidence.
#[test]
fn replacement_cli_uses_session_feature_projection_and_persists_semantic_module_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let source_root = temporary.path().join("src");
    fs::create_dir_all(&source_root)?;
    let entrypoint = source_root.join("main.incn");
    fs::write(
        temporary.path().join("incan.toml"),
        "[project]\nname = \"replacement_session_projection\"\n\n[project.features]\nbeta = []\n",
    )?;
    fs::write(
        &entrypoint,
        "when feature(\"beta\"):\n  def main() -> int:\n    return 42\n",
    )?;

    let inactive = Command::new(incan_binary())
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
        !inactive.status.success(),
        "an inactive session projection must not execute its gated entrypoint"
    );
    assert!(
        !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a refused inactive entrypoint must not publish a replacement receipt"
    );

    let enabled = Command::new(incan_binary())
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--features",
            "beta",
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
        enabled.status.success(),
        "the selected session feature must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&enabled.stdout),
        String::from_utf8_lossy(&enabled.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    let provenance = &report["semantic_module"];
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert_eq!(provenance["module_id"], "module:main");
    assert_eq!(provenance["module_path"], "main");
    assert!(
        provenance["source_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:"))
            && provenance["semantic_snapshot_identity"]
                .as_str()
                .is_some_and(|identity| identity.starts_with("sha256:")),
        "the report must name stable source and semantic-snapshot identities: {report}"
    );
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| snapshot.contains("span="))
            && report["replacement_execution"]["ownership_reads"].is_array()
            && report["replacement_execution"]["runtime_requirements"].is_array(),
        "the execution evidence must stay attached to the session-owned semantic module: {report}"
    );

    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["semantic_module"], *provenance);
    assert_eq!(receipt["selection"]["source_identity"], provenance["source_identity"]);
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a session-projected replacement execution must not create legacy generated output"
    );
    Ok(())
}

/// The only admitted import is `std.async` activation; its direct execution projects receipt-bound task evidence
/// without creating generated Rust or claiming a source-observable comparison.
#[test]
fn replacement_cli_executes_exact_std_async_activation_with_task_receipts() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"import std.async

async def child() -> int:
  return 7

async def main() -> int:
  return await child()
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
        "direct async replacement build must succeed. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["backend"]["fallback_outcome"], "not_needed");
    assert_eq!(report["replacement_execution"]["result"], "7");
    assert!(
        report["replacement_execution"]["task_lifecycle"].is_array()
            && report["replacement_execution"]["task_lifecycle"]
                .as_array()
                .is_some_and(|events| events.iter().any(|event| event["event"] == "await_resumed")),
        "the direct execution report must project receipt-bound task lifecycle evidence: {report}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "the direct async profile must not create a generated legacy project"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert_eq!(receipt["shadow_comparison"], "not_requested");
    Ok(())
}

/// A direct child-task failure neither falls back nor writes a success receipt.
#[test]
fn replacement_cli_refuses_async_task_failure_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"import std.async

async def child() -> int:
  assert false
  return 7

async def main() -> int:
  return await child()
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
        "a failing direct child task must not fall back to legacy execution"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("INCAN-R988-RUNTIME") && combined.contains("assertion failed"),
        "the original direct task failure must remain visible: {combined}"
    );
    let assertion_start = source
        .find("assert false")
        .ok_or("fixture must contain the child assertion")?;
    let assertion_end = assertion_start + "assert false".len();
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{assertion_start}..{assertion_end}",
            entrypoint.display()
        )),
        "the CLI error must retain the child assertion's original source span: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a failing direct task must not create generated output or a misleading receipt"
    );
    Ok(())
}

/// A failing source-order race winner cannot leave an unpolled loser on a legacy or receipt-producing path.
#[test]
fn replacement_cli_refuses_failing_race_winner_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"import std.async

async def first() -> int:
  assert false
  return 1

async def loser() -> int:
  assert false
  return 2

async def main() -> int:
  winner = race for value:
    await first() => value
    await loser() => value
  return winner
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
        "a failing source-order race winner must not fall back to legacy execution"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("INCAN-R988-RUNTIME") && combined.contains("assertion failed"),
        "the original direct race winner failure must remain visible: {combined}"
    );
    let assertion_start = source
        .find("assert false")
        .ok_or("fixture must contain the first race-arm assertion")?;
    let assertion_end = assertion_start + "assert false".len();
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{assertion_start}..{assertion_end}",
            entrypoint.display()
        )),
        "the CLI error must retain the first race-arm assertion span: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a failing race winner must not create generated output or a misleading receipt"
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
  values = {**{"first": 1}}
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
        combined.contains("`unsafe:` acknowledgement region") && combined.contains("original Incan source span"),
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
            "rust.module(\"incan_stdlib::testing\")\n\ndef main() -> int:\n  return 42\n",
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

/// Duplicate standard-library activation is a canonical binding error before backend selection begins.
#[test]
fn duplicate_async_activation_fails_during_typechecking() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\nimport std.async\n\ndef main() -> int:\n  return 42\n";
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("duplicate-async-activation.incn");
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
    assert!(!output.status.success(), "duplicate activation must not compile");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Duplicate definition of 'async'")
            && combined.contains("First canonical identity:")
            && combined.contains("Second canonical identity:"),
        "duplicate activation must report its conflicting canonical bindings: {combined}"
    );
    assert!(
        !combined.contains("INCAN-R988-UNSUPPORTED"),
        "backend refusal must not replace the earlier type error: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a compile-time binding error must not create backend output or a receipt"
    );
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
    // at their first call. It is now resolved through `incan_core`'s builtin registry rather than by name, and the
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
    let rust = incan::backend::IrCodegen::new()
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
    // Printing shares one rendering rule with f-string interpolation, so it inherits the same boundary: a value
    // whose spelling this runtime and the Rust-emission backend do not provably agree on refuses instead of
    // guessing.
    let source = r#"
def main() -> None:
  println([1, 2])
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
