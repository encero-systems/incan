//! Boundary tests for canonical `enumerate` and `zip` direct execution.

use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::diagnostics::CompileError;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_core::lang::builtins::BuiltinFnId;
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParamDefault, CallableTarget, Callee, IterProtocol, LocalDecl, LocalId, Rvalue,
    Statement, StatementKind,
};
use incan_semantics_core::{HirSourceSpan, IncanPrimitiveType, IncanType};

/// Lower self-contained source through the checked Body IR consumed by direct execution.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_enumerate_zip_boundary".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Retain frontend diagnostics when malformed source stops before Body IR is available.
fn check_source(source: &str) -> Result<Vec<CompileError>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    match checker.check_program(&program) {
        Ok(()) => Ok(Vec::new()),
        Err(errors) => Ok(errors),
    }
}

/// Establish that a fixture is executable before corrupting only its checked Body-IR metadata.
fn assert_uncorrupted_result(
    module: &BodyIrModule,
    function: &str,
    value: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let execution = execute_free_function(module, function, &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(value));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// Find one named direct body without relying on its position in the module.
fn named_body_mut<'a>(module: &'a mut BodyIrModule, name: &str) -> Result<&'a mut Body, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter_mut()
        .find(|body| body.name == name)
        .ok_or_else(|| format!("fixture must lower body `{name}`").into())
}

/// Look up a checked local declaration by its source name.
fn named_local_mut<'a>(body: &'a mut Body, name: &str) -> Result<&'a mut LocalDecl, Box<dyn std::error::Error>> {
    body.locals
        .iter_mut()
        .find(|local| local.name.as_deref() == Some(name))
        .ok_or_else(|| format!("fixture must retain checked local `{name}`").into())
}

/// Look up one compiler-created or source local by its retained Body-IR identity.
fn local_by_id_mut(body: &mut Body, id: LocalId) -> Result<&mut LocalDecl, Box<dyn std::error::Error>> {
    body.locals
        .iter_mut()
        .find(|local| local.id == id)
        .ok_or_else(|| format!("fixture must retain local _{}", id.0).into())
}

/// Find an exact builtin call destination through normal and deferred statement trees.
fn builtin_call_destination(body: &Body, builtin: BuiltinFnId) -> Option<LocalId> {
    for parameter in &body.params {
        if let CallableParamDefault::Source(computation) = &parameter.default
            && let Some(destination) = builtin_call_destination_in_statements(&computation.stmts, builtin)
        {
            return Some(destination);
        }
    }
    builtin_call_destination_in_statements(&body.block.stmts, builtin)
}

/// Find a canonical builtin call destination without using a source spelling as identity.
fn builtin_call_destination_in_statements(statements: &[Statement], builtin: BuiltinFnId) -> Option<LocalId> {
    for statement in statements {
        if let StatementKind::Call {
            destination: Some(destination),
            callee: Callee::Function(CallableTarget::Named(target)),
            ..
        } = &statement.kind
            && target.direct_call_id.is_none()
            && target.builtin == Some(builtin)
            && destination.projection.is_empty()
            && let Some(local) = destination.local_id()
        {
            return Some(local);
        }
        if let Some(destination) = builtin_call_destination_in_nested_statement(statement, builtin) {
            return Some(destination);
        }
    }
    None
}

/// Recurse through the checked deferred shapes that retain calls under an owning Body local table.
fn builtin_call_destination_in_nested_statement(statement: &Statement, builtin: BuiltinFnId) -> Option<LocalId> {
    match &statement.kind {
        StatementKind::If {
            then_block, else_block, ..
        } => builtin_call_destination_in_statements(&then_block.stmts, builtin).or_else(|| {
            else_block
                .as_ref()
                .and_then(|block| builtin_call_destination_in_statements(&block.stmts, builtin))
        }),
        StatementKind::Loop { body } => builtin_call_destination_in_statements(&body.stmts, builtin),
        StatementKind::Race { arms, .. } => arms
            .iter()
            .find_map(|arm| builtin_call_destination_in_statements(&arm.body.stmts, builtin)),
        StatementKind::Assign { rvalue, .. } => builtin_call_destination_in_rvalue(rvalue, builtin),
        _ => None,
    }
}

/// Recurse through the deferred rvalues that own executable statements.
fn builtin_call_destination_in_rvalue(rvalue: &Rvalue, builtin: BuiltinFnId) -> Option<LocalId> {
    match rvalue {
        Rvalue::Closure { params, body, .. } => {
            for parameter in params {
                if let CallableParamDefault::Source(computation) = &parameter.default
                    && let Some(destination) = builtin_call_destination_in_statements(&computation.stmts, builtin)
                {
                    return Some(destination);
                }
            }
            builtin_call_destination_in_statements(&body.stmts, builtin)
        }
        Rvalue::Generator { body, .. } => builtin_call_destination_in_statements(&body.stmts, builtin),
        Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
            builtin_call_destination_in_statements(&arm.guard_stmts, builtin)
                .or_else(|| builtin_call_destination_in_statements(&arm.body_stmts, builtin))
        }),
        _ => None,
    }
}

/// Clear one checked builtin fact through the same normal/deferred traversal used by execution admission.
fn clear_builtin_identity(body: &mut Body, builtin: BuiltinFnId) -> bool {
    for parameter in &mut body.params {
        if let CallableParamDefault::Source(computation) = &mut parameter.default
            && clear_builtin_identity_in_statements(&mut computation.stmts, builtin)
        {
            return true;
        }
    }
    clear_builtin_identity_in_statements(&mut body.block.stmts, builtin)
}

/// Clear one canonical builtin fact without consulting the target's source spelling.
fn clear_builtin_identity_in_statements(statements: &mut [Statement], builtin: BuiltinFnId) -> bool {
    for statement in statements {
        if let StatementKind::Call {
            callee: Callee::Function(CallableTarget::Named(target)),
            ..
        } = &mut statement.kind
            && target.direct_call_id.is_none()
            && target.builtin == Some(builtin)
        {
            target.builtin = None;
            return true;
        }
        if clear_builtin_identity_in_nested_statement(statement, builtin) {
            return true;
        }
    }
    false
}

/// Continue an identity corruption search through nested control flow and deferred rvalues.
fn clear_builtin_identity_in_nested_statement(statement: &mut Statement, builtin: BuiltinFnId) -> bool {
    match &mut statement.kind {
        StatementKind::If {
            then_block, else_block, ..
        } => {
            clear_builtin_identity_in_statements(&mut then_block.stmts, builtin)
                || else_block
                    .as_mut()
                    .is_some_and(|block| clear_builtin_identity_in_statements(&mut block.stmts, builtin))
        }
        StatementKind::Loop { body } => clear_builtin_identity_in_statements(&mut body.stmts, builtin),
        StatementKind::Race { arms, .. } => arms
            .iter_mut()
            .any(|arm| clear_builtin_identity_in_statements(&mut arm.body.stmts, builtin)),
        StatementKind::Assign { rvalue, .. } => clear_builtin_identity_in_rvalue(rvalue, builtin),
        _ => false,
    }
}

/// Continue an identity corruption search through deferred closure, generator, and match bodies.
fn clear_builtin_identity_in_rvalue(rvalue: &mut Rvalue, builtin: BuiltinFnId) -> bool {
    match rvalue {
        Rvalue::Closure { params, body, .. } => {
            for parameter in params {
                if let CallableParamDefault::Source(computation) = &mut parameter.default
                    && clear_builtin_identity_in_statements(&mut computation.stmts, builtin)
                {
                    return true;
                }
            }
            clear_builtin_identity_in_statements(&mut body.stmts, builtin)
        }
        Rvalue::Generator { body, .. } => clear_builtin_identity_in_statements(&mut body.stmts, builtin),
        Rvalue::Match { arms, .. } => arms.iter_mut().any(|arm| {
            clear_builtin_identity_in_statements(&mut arm.guard_stmts, builtin)
                || clear_builtin_identity_in_statements(&mut arm.body_stmts, builtin)
        }),
        _ => false,
    }
}

/// Find the exact destination and source span of one retained builtin poll across deferred frames.
fn builtin_iter_next_destination(body: &Body) -> Option<(LocalId, HirSourceSpan)> {
    for parameter in &body.params {
        if let CallableParamDefault::Source(computation) = &parameter.default
            && let Some(location) = builtin_iter_next_destination_in_statements(&computation.stmts)
        {
            return Some(location);
        }
    }
    builtin_iter_next_destination_in_statements(&body.block.stmts)
}

/// Find one `IterNext` whose checked protocol is builtin iteration, including nested deferred bodies.
fn builtin_iter_next_destination_in_statements(statements: &[Statement]) -> Option<(LocalId, HirSourceSpan)> {
    for statement in statements {
        if let StatementKind::IterNext {
            destination,
            protocol: IterProtocol::Builtin,
            ..
        } = &statement.kind
            && destination.projection.is_empty()
            && let Some(local) = destination.local_id()
        {
            return Some((local, statement.span));
        }
        let nested = match &statement.kind {
            StatementKind::If {
                then_block, else_block, ..
            } => builtin_iter_next_destination_in_statements(&then_block.stmts).or_else(|| {
                else_block
                    .as_ref()
                    .and_then(|block| builtin_iter_next_destination_in_statements(&block.stmts))
            }),
            StatementKind::Loop { body } => builtin_iter_next_destination_in_statements(&body.stmts),
            StatementKind::Race { arms, .. } => arms
                .iter()
                .find_map(|arm| builtin_iter_next_destination_in_statements(&arm.body.stmts)),
            StatementKind::Assign { rvalue, .. } => match rvalue {
                Rvalue::Closure { body, .. } => builtin_iter_next_destination_in_statements(&body.stmts),
                Rvalue::Generator { body, .. } => builtin_iter_next_destination_in_statements(&body.stmts),
                Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
                    builtin_iter_next_destination_in_statements(&arm.guard_stmts)
                        .or_else(|| builtin_iter_next_destination_in_statements(&arm.body_stmts))
                }),
                _ => None,
            },
            _ => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

/// Corrupt the checked item-local type at an actual builtin polling node and retain that node's source span.
fn corrupt_builtin_iter_next_destination(body: &mut Body) -> Result<HirSourceSpan, Box<dyn std::error::Error>> {
    let (destination, span) =
        builtin_iter_next_destination(body).ok_or("fixture must lower an IterNext with the builtin protocol")?;
    local_by_id_mut(body, destination)?.ty = IncanType::Primitive(IncanPrimitiveType::Int);
    Ok(span)
}

/// Find the original assignment span for one bare local through normalized control flow and deferred frames.
fn assignment_destination_span(statements: &[Statement], destination: LocalId) -> Option<HirSourceSpan> {
    for statement in statements {
        if let StatementKind::Assign { place, .. } = &statement.kind
            && place.projection.is_empty()
            && place.local_id() == Some(destination)
        {
            return Some(statement.span);
        }
        let nested = match &statement.kind {
            StatementKind::If {
                then_block, else_block, ..
            } => assignment_destination_span(&then_block.stmts, destination).or_else(|| {
                else_block
                    .as_ref()
                    .and_then(|block| assignment_destination_span(&block.stmts, destination))
            }),
            StatementKind::Loop { body } => assignment_destination_span(&body.stmts, destination),
            StatementKind::Race { arms, .. } => arms
                .iter()
                .find_map(|arm| assignment_destination_span(&arm.body.stmts, destination)),
            StatementKind::Assign { rvalue, .. } => match rvalue {
                Rvalue::Closure { body, .. } => assignment_destination_span(&body.stmts, destination),
                Rvalue::Generator { body, .. } => assignment_destination_span(&body.stmts, destination),
                Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
                    assignment_destination_span(&arm.guard_stmts, destination)
                        .or_else(|| assignment_destination_span(&arm.body_stmts, destination))
                }),
                _ => None,
            },
            _ => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

/// Require a direct-profile refusal to cite the exact original call expression.
fn assert_direct_refusal_at_call(
    module: &BodyIrModule,
    function: &str,
    source: &str,
    call: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = source
        .find(call)
        .ok_or_else(|| format!("fixture must contain rejected call `{call}`"))?;
    assert_direct_refusal_at_span(module, function, HirSourceSpan::new(start, start + call.len()))
}

/// Require a direct-profile refusal to retain one exact source span already selected by Body IR.
fn assert_direct_refusal_at_span(
    module: &BodyIrModule,
    function: &str,
    expected_span: HirSourceSpan,
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
    let span = error
        .primary_span()
        .ok_or("direct refusal must retain an original source span")?;
    assert_eq!(span, expected_span);
    Ok(())
}

/// Accept either the retained frontend diagnostic or the direct-profile refusal at the original source call.
fn assert_refusal_or_frontend_diagnostic(
    source: &str,
    function: &str,
    call: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = source
        .find(call)
        .ok_or_else(|| format!("fixture must contain rejected call `{call}`"))?;
    let end = start + call.len();
    let errors = check_source(source)?;
    if errors.is_empty() {
        let module = lower_typed_body_ir(source)?;
        return assert_direct_refusal_at_call(&module, function, source, call);
    }
    let diagnostic = errors
        .iter()
        .find(|error| {
            (error.span.start <= start && end <= error.span.end) || (start <= error.span.start && error.span.end <= end)
        })
        .ok_or_else(|| format!("{call} must diagnose its original call span, got {errors:?}"))?;
    assert!(
        (diagnostic.span.start <= start && end <= diagnostic.span.end)
            || (start <= diagnostic.span.start && diagnostic.span.end <= end)
    );
    Ok(())
}

/// Construct `list[list[float]]` without deriving a type from a runtime-empty value.
fn nested_float_list_type() -> IncanType {
    IncanType::Generic {
        base: "list".to_string(),
        args: vec![IncanType::Generic {
            base: "list".to_string(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Float)],
        }],
    }
}

/// Missing canonical Zip identity must not be reconstructed from the source spelling.
#[test]
fn replacement_refuses_zip_without_its_checked_builtin_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def missing_zip_identity() -> int:
  zip([1], [2])
  return 0
"#;
    let mut module = lower_typed_body_ir(source)?;
    let body = named_body_mut(&mut module, "missing_zip_identity")?;
    assert!(clear_builtin_identity(body, BuiltinFnId::Zip));

    assert_direct_refusal_at_call(&module, "missing_zip_identity", source, "zip([1], [2])")
}

/// Empty arguments cannot evade the checked operand and destination contracts after Body IR is corrupted.
#[test]
fn replacement_refuses_corrupted_zip_operand_and_nominal_iterator_destination() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def typed_zip_target(left: list[list[int]], right: list[int]) -> int:
  pairs = zip(left, right)
  mut total = 0
  for typed_zip_left, typed_zip_right in pairs:
    total += len(typed_zip_left) + typed_zip_right
  return total

def typed_zip_entry() -> int:
  empty_left: list[list[int]] = []
  empty_right: list[int] = []
  return typed_zip_target(empty_left, empty_right)
"#;
    let call = "zip(left, right)";

    let mut operand_module = lower_typed_body_ir(source)?;
    let operand_body = named_body_mut(&mut operand_module, "typed_zip_target")?;
    named_local_mut(operand_body, "left")?.ty = nested_float_list_type();
    assert_direct_refusal_at_call(&operand_module, "typed_zip_entry", source, call)?;

    let mut destination_module = lower_typed_body_ir(source)?;
    let destination_body = named_body_mut(&mut destination_module, "typed_zip_target")?;
    let destination = builtin_call_destination(destination_body, BuiltinFnId::Zip)
        .ok_or("fixture must retain Zip's canonical call destination")?;
    local_by_id_mut(destination_body, destination)?.ty = IncanType::Named("Iterator".to_string());
    assert_direct_refusal_at_call(&destination_module, "typed_zip_entry", source, call)
}

/// An alias that changes a canonical Zip iterator's checked type refuses at its own assignment span.
#[test]
fn replacement_refuses_a_zip_alias_with_a_corrupted_checked_iterator_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def zip_alias_type_mismatch() -> int:
  pairs = zip([1], [2])
  alias = pairs
  mut total = 0
  for alias_left, alias_right in alias:
    total += alias_left + alias_right
  return total
"#;
    let mut module = lower_typed_body_ir(source)?;
    let body = named_body_mut(&mut module, "zip_alias_type_mismatch")?;
    let alias = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("alias"))
        .map(|local| local.id)
        .ok_or("fixture must retain the Zip alias local")?;
    let assignment_span =
        assignment_destination_span(&body.block.stmts, alias).ok_or("fixture must retain the Zip alias assignment")?;
    local_by_id_mut(body, alias)?.ty = IncanType::Named("Iterator".to_string());

    assert_direct_refusal_at_span(&module, "zip_alias_type_mismatch", assignment_span)
}

/// Recursively structural list and tuple leaves remain executable without nominal iterator admission.
#[test]
fn replacement_executes_zip_with_recursively_structural_nested_lists_and_tuples()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def nested_structural_zip() -> int:
  left = [[(1, "one")], [(2, "two")]]
  right = [("first", true), ("second", false)]
  pairs = zip(left, right)
  mut total = 0
  for nested_left_group, nested_right_pair in pairs:
    for nested_member in nested_left_group:
      total += nested_member.0
    if nested_right_pair.1:
      total += 10
  return total
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "nested_structural_zip", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(13));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    assert!(execution.output_identity.starts_with("sha256:"));
    Ok(())
}

/// Zip must preserve original spans for non-list values, malformed arity, and parser-admitted spread forms.
#[test]
fn replacement_refuses_zip_shape_boundaries_at_the_original_source_span() -> Result<(), Box<dyn std::error::Error>> {
    let nonlist_source = r#"
def zip_nonlist_boundary() -> int:
  pairs = zip("not a list", [1])
  return 0
"#;
    assert_refusal_or_frontend_diagnostic(nonlist_source, "zip_nonlist_boundary", "zip(\"not a list\", [1])")?;

    let arity_source = r#"
def zip_arity_boundary() -> int:
  pairs = zip([1])
  return 0
"#;
    assert_refusal_or_frontend_diagnostic(arity_source, "zip_arity_boundary", "zip([1])")?;

    let spread_source = r#"
def zip_spread_boundary() -> int:
  inputs = [[1], [2]]
  pairs = zip(*inputs)
  return 0
"#;
    assert_refusal_or_frontend_diagnostic(spread_source, "zip_spread_boundary", "zip(*inputs)")
}

/// A source-defined `zip` keeps its checked direct-call identity rather than borrowing the builtin profile.
#[test]
fn replacement_preserves_source_defined_zip_over_the_global_builtin_spelling() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def zip(left: list[int], right: list[int]) -> int:
  println("source zip")
  return 42

def source_defined_zip() -> int:
  return zip([1], [2])
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "source_defined_zip", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert_eq!(execution.output.stdout(), b"source zip\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

/// A stored closure consumes a captured canonical Zip value through its retained closure-frame local.
#[test]
fn replacement_executes_a_closure_that_captures_canonical_zip() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def captured_zip_closure() -> int:
  outer_pairs = zip([4], [5])
  closure_values: () -> list[int] = () => (captured_left + captured_right for captured_left, captured_right in outer_pairs).collect()
  captured_values = closure_values()
  captured_first = captured_values[0]
  return captured_first
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "captured_zip_closure", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(9));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    assert!(execution.body_snapshot.contains("executed stored callable frame"));
    Ok(())
}

/// Later generator clauses consume captured canonical Enumerate and Zip iterables rather than source-name guesses.
#[test]
fn replacement_executes_later_generator_clauses_over_captured_enumerate_and_zip()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def later_clause_generator_capture() -> int:
  captured_enumeration = enumerate([1])
  captured_zip = zip([2], [3])
  generated_values = (outer_value + later_index + later_value + later_left + later_right for outer_value in [0] for later_index, later_value in captured_enumeration for later_left, later_right in captured_zip).collect()
  generated_first = generated_values[0]
  return generated_first
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "later_clause_generator_capture", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(6));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    assert!(execution.body_snapshot.contains("executed generator-expression frame"));
    Ok(())
}

/// Every retained Enumerate poll rechecks its own checked item local, including deferred frames.
#[test]
fn replacement_refuses_corrupted_enumerate_iter_next_destinations_in_top_level_and_deferred_frames()
-> Result<(), Box<dyn std::error::Error>> {
    let top_level_source = r#"
def top_level_enumerate_item() -> int:
  pairs = enumerate([1])
  mut total = 0
  for top_level_index, top_level_value in pairs:
    total += top_level_index + top_level_value
  return total
"#;
    let mut top_level_module = lower_typed_body_ir(top_level_source)?;
    assert_uncorrupted_result(&top_level_module, "top_level_enumerate_item", 1)?;
    let top_level_body = named_body_mut(&mut top_level_module, "top_level_enumerate_item")?;
    assert!(
        builtin_call_destination(top_level_body, BuiltinFnId::Enumerate).is_some(),
        "fixture must retain canonical Enumerate construction before corrupting its poll item"
    );
    let top_level_span = corrupt_builtin_iter_next_destination(top_level_body)?;
    assert_direct_refusal_at_span(&top_level_module, "top_level_enumerate_item", top_level_span)?;

    let generator_source = r#"
def generated_enumerate_item() -> Generator[int]:
  for generated_index, generated_value in enumerate([5]):
    yield generated_index + generated_value

def generated_enumerate_entry() -> int:
  generated_values = generated_enumerate_item().collect()
  generated_first = generated_values[0]
  return generated_first
"#;
    let mut generator_module = lower_typed_body_ir(generator_source)?;
    assert_uncorrupted_result(&generator_module, "generated_enumerate_entry", 5)?;
    let generator_body = named_body_mut(&mut generator_module, "generated_enumerate_item")?;
    assert!(
        builtin_call_destination(generator_body, BuiltinFnId::Enumerate).is_some(),
        "named generator body must retain canonical Enumerate construction"
    );
    let generator_span = corrupt_builtin_iter_next_destination(generator_body)?;
    assert_direct_refusal_at_span(&generator_module, "generated_enumerate_entry", generator_span)?;

    let expression_generator_source = r#"
def expression_generator_enumerate_entry() -> int:
  expression_values = (expression_pair.0 for expression_pair in enumerate([8])).collect()
  expression_first = expression_values[0]
  return expression_first
"#;
    let expression_generator_normal = lower_typed_body_ir(expression_generator_source)?;
    let normal_execution = execute_free_function(
        &expression_generator_normal,
        "expression_generator_enumerate_entry",
        &[],
    )?;
    assert_eq!(normal_execution.value, ReplacementValue::Int(0));
    assert!(normal_execution.output.stdout().is_empty());
    assert!(normal_execution.output.stderr().is_empty());

    let mut expression_generator_module = lower_typed_body_ir(expression_generator_source)?;
    let expression_generator_body =
        named_body_mut(&mut expression_generator_module, "expression_generator_enumerate_entry")?;
    let expression_generator_span = corrupt_builtin_iter_next_destination(expression_generator_body)?;
    assert_direct_refusal_at_span(
        &expression_generator_module,
        "expression_generator_enumerate_entry",
        expression_generator_span,
    )?;

    let closure_source = r#"
def closure_enumerate_entry() -> int:
  closure_values: () -> list[int] = () => (closure_value.1 for closure_value in enumerate([6])).collect()
  closure_result = closure_values()
  return closure_result[0]
"#;
    let mut closure_module = lower_typed_body_ir(closure_source)?;
    assert_uncorrupted_result(&closure_module, "closure_enumerate_entry", 6)?;
    let closure_body = named_body_mut(&mut closure_module, "closure_enumerate_entry")?;
    assert!(
        builtin_call_destination(closure_body, BuiltinFnId::Enumerate).is_some(),
        "closure body must retain canonical Enumerate construction"
    );
    let closure_span = corrupt_builtin_iter_next_destination(closure_body)?;
    assert_direct_refusal_at_span(&closure_module, "closure_enumerate_entry", closure_span)?;

    let default_source = r#"
def default_enumerate_item(values: list[int] = (default_value.1 for default_value in enumerate([7])).collect()) -> int:
  return values[0]

def default_enumerate_entry() -> int:
  return default_enumerate_item()
"#;
    let mut default_module = lower_typed_body_ir(default_source)?;
    assert_uncorrupted_result(&default_module, "default_enumerate_entry", 7)?;
    let default_body = named_body_mut(&mut default_module, "default_enumerate_item")?;
    assert!(
        builtin_call_destination(default_body, BuiltinFnId::Enumerate).is_some(),
        "source default must retain canonical Enumerate construction"
    );
    let default_span = corrupt_builtin_iter_next_destination(default_body)?;
    assert_direct_refusal_at_span(&default_module, "default_enumerate_entry", default_span)
}

/// Canonical calls in deferred defaults, closures, and generator frames use their own retained checked facts.
#[test]
fn replacement_executes_deferred_enumerate_and_zip_frames_with_distinct_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def defaulted_enumeration(values: list[int] = (default_pair.1 for default_pair in enumerate([7])).collect()) -> int:
  return values[0]

def generated_zip_values() -> Generator[int]:
  for generator_left, generator_right in zip([4], [5]):
    yield generator_left + generator_right

def deferred_enumerate_zip() -> int:
  closure_values: () -> list[int] = () => (closure_left + closure_right for closure_left, closure_right in zip([2], [3])).collect()
  closure_result = closure_values()
  generated_values = generated_zip_values().collect()
  generated_first = generated_values[0]
  return defaulted_enumeration() + closure_result[0] + generated_first
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "deferred_enumerate_zip", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(21));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    assert!(
        execution.body_snapshot.contains("executed source default frame")
            && execution.body_snapshot.contains("executed stored callable frame")
            && execution.body_snapshot.contains("executed generator-function frame"),
        "all executed deferred frames must remain receipt-bound Body-IR evidence: {}",
        execution.body_snapshot
    );
    Ok(())
}
