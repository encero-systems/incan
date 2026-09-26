//! Source-local nominal models and enums through a direct callable: construction and checked field layout,
//! identity-selected match patterns, same-error result routing, value and fieldless members, and the refusals
//! when a retained identity, a checked field layout, a default or a canonical member no longer matches the
//! physical record.

use super::*;

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
            incan_driver::backend::replacement::ReplacementExecutionError::Unsupported {
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
            incan_driver::backend::replacement::ReplacementExecutionError::Unsupported {
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
            .contains("declaration context is absent from the execution graph"),
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
            .contains("declaration context is absent from the execution graph"),
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

    assert_malformed_pair_constructor_refusal(&module, "declaration context is absent from the execution graph")
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
