//! Provider-operation plans (#1213): admitted calls lower to checked execution plans with evaluated-input facts,
//! authority requests and published runtime requirements; unadmitted, disabled, unavailable, duplicate, corrupt and
//! unresolved operations refuse at their source span before lowering arguments.

use super::*;

/// Lower `source` through its checked provider manifest and the selected `ProviderPlan`.
fn build_with_provider_operation(
    source: &str,
    module_path: &[&str],
    state: bir::ProviderActivationState,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let provider_plan = provider_plan_from_checked_source(checker.type_info(), state)?;
    build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)
        .map_err(Into::into)
}

/// Collect the provider-operation plans lowered into one body, in statement order.
fn provider_plans<'module>(
    module: &'module bir::BodyIrModule,
    body_name: &str,
) -> Vec<&'module bir::ProviderOperationPlan> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::ProviderOperation(plan),
                ..
            } => Some(plan.as_ref()),
            _ => None,
        })
        .collect()
}

/// Collect the explicit refusal descriptions recorded in one body, in statement order.
fn refusal_descriptions<'module>(module: &'module bir::BodyIrModule, body_name: &str) -> Vec<&'module str> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Unsupported { description } => Some(description.as_str()),
            _ => None,
        })
        .collect()
}

const PROVIDER_FIXTURE_SOURCE: &str = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int) -> int:
  return amount

def run() -> int:
  return charge("acct-1", 250)
"#;

/// An admitted call reaches Body IR as a plan carrying every checked fact #1156 needs to execute it.
#[test]
fn an_admitted_provider_operation_lowers_to_a_checked_execution_plan() -> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from(format!(
            "expected one provider-operation plan, got {}",
            plans.len()
        )));
    };

    assert_eq!(plan.operation.declaration_name, "charge");
    assert_eq!(
        plan.operation.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["app".to_string()]),
        "the plan must name the declaration the call resolved to, not the call-site spelling"
    );
    assert_eq!(plan.provider.state, bir::ProviderActivationState::Active);
    assert_eq!(
        plan.provider.provider_key,
        "fixture_provider@0.1.0#fixture:provider-operation[]"
    );
    assert_eq!(
        plan.required_capability.kind,
        SemanticSourceTargetKind::Capability,
        "the plan's required authority must name a capability declaration"
    );
    assert!(
        plan.runtime_requirements.is_empty(),
        "the first provider-operation contract does not infer runtime requirements from a source spelling"
    );

    // The declaration's own body must still lower, and the plan must name it rather than shadow it.
    let declaration = module
        .body_for_canonical_target(&plan.operation)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("this module owns the operation and must resolve it"))?;
    assert_eq!(declaration.name, "charge");
    Ok(())
}

/// Consumer lowering uses the provider manifest's declaration identity, even though the caller only wrote an import.
#[test]
fn an_imported_provider_operation_lowers_from_the_selected_provider_manifest() -> Result<(), Box<dyn std::error::Error>>
{
    let provider_source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
pub def charge(account: str, amount: int) -> int:
  return amount
"#;
    let consumer_source = r#"
from provider import charge

def run() -> int:
  return charge("acct-1", 250)
"#;
    let provider_tokens = lexer::lex(provider_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let provider_program =
        parser::parse(&provider_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut provider_checker = TypeChecker::new();
    provider_checker.set_current_module_path(Some(vec!["provider".to_string()]));
    provider_checker
        .check_program(&provider_program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let provider_plan =
        provider_plan_from_checked_source(provider_checker.type_info(), bir::ProviderActivationState::Active)?;

    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let consumer_program =
        parser::parse(&consumer_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut consumer_checker = TypeChecker::new();
    let consumer_module_path = vec!["app".to_string()];
    consumer_checker.set_current_module_path(Some(consumer_module_path.clone()));
    consumer_checker.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    consumer_checker
        .check_with_imports(&consumer_program, &[("provider", &provider_program)])
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    assert!(
        consumer_checker
            .type_info()
            .declarations
            .function_bindings
            .contains_key("charge"),
        "an imported function must retain its checked signature for argument planning"
    );

    let module = build_body_ir_module_v0_with_provider_plan(
        &consumer_program,
        &consumer_module_path,
        consumer_checker.type_info(),
        &provider_plan,
    )?;
    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from(format!(
            "expected one imported provider plan, got {}; lowered body: {}",
            plans.len(),
            module.render_snapshot()
        )));
    };
    assert_eq!(
        plan.operation.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["provider".to_string()]),
        "the plan must preserve the provider declaration identity, not the consumer import spelling"
    );
    assert_eq!(plan.required_capability.declaration_name, "charge_card");
    assert_eq!(plan.provider.module_path, vec!["provider".to_string()]);
    Ok(())
}

/// The plan describes each evaluated input by slot, evaluation order, checked type, and its own span.
#[test]
fn a_plan_records_every_evaluated_input_fact() -> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from("expected one provider-operation plan".to_string()));
    };
    let [account, amount] = plan.inputs.as_slice() else {
        return Err(Box::from(format!(
            "expected two evaluated inputs, got {}",
            plan.inputs.len()
        )));
    };

    assert_eq!((account.slot, account.written_position), (0, 0));
    assert_eq!((amount.slot, amount.written_position), (1, 1));
    assert_eq!(account.ty, IncanType::Primitive(IncanPrimitiveType::Str));
    assert_eq!(amount.ty, IncanType::Primitive(IncanPrimitiveType::Int));
    assert!(
        account.span.start < amount.span.start && amount.span.end <= plan.call_span.end,
        "each input must carry its own argument span inside the call: {:?}",
        plan.inputs
    );
    Ok(())
}

/// Named provider arguments preserve source evaluation order even though their operands are emitted by declaration
/// slot. The plan is the bridge between those two orders, so losing either fact would let a consumer reorder effects
/// or ownership decisions.
#[test]
fn a_provider_plan_preserves_reversed_named_argument_evaluation_and_ownership() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, memo: str) -> int:
  return 1

def first() -> str:
  return "first"

def second() -> str:
  return "second"

def run() -> int:
  return charge(memo=first(), account=second())
"#;
    let module = build_with_provider_operation(source, &["app"], bir::ProviderActivationState::Active)?;
    let body = body_named(&module, "run")?;
    let rendered = body.render_snapshot();
    let first_at = rendered
        .find("call fn:first(")
        .ok_or("missing first argument evaluation")?;
    let second_at = rendered
        .find("call fn:second(")
        .ok_or("missing second argument evaluation")?;
    assert!(
        first_at < second_at,
        "named arguments must evaluate in written source order: {rendered}"
    );

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from("expected one provider-operation plan".to_string()));
    };
    assert_eq!(
        plan.inputs
            .iter()
            .map(|input| (input.slot, input.written_position))
            .collect::<Vec<_>>(),
        vec![(1, 0), (0, 1)],
        "the plan must retain written order separately from declaration slots: {:?}",
        plan.inputs
    );

    let provider_call = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::ProviderOperation(_),
                args,
                ..
            } => Some(args),
            _ => None,
        })
        .ok_or("missing provider-operation call")?;
    let [
        bir::ArgumentElement::One(bir::Operand::Place(account)),
        bir::ArgumentElement::One(bir::Operand::Place(memo)),
    ] = provider_call.as_slice()
    else {
        return Err(Box::from(format!(
            "expected two declared-slot provider operands, got {provider_call:?}"
        )));
    };
    assert_eq!(
        (account.fact, account.last_use, memo.fact, memo.last_use),
        (bir::OwnershipFact::Move, true, bir::OwnershipFact::Move, true),
        "the declaration-slot operands must retain the ownership facts from their written evaluations"
    );
    Ok(())
}

/// The plan phrases the RFC 104 authority question at the invocation, which is what #1156 executes from.
#[test]
fn a_plan_yields_an_authority_request_for_the_invocation() -> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from("expected one provider-operation plan".to_string()));
    };
    let request = plan.authority_request();

    assert_eq!(request.capability, plan.required_capability);
    assert_eq!(request.operation, plan.operation);
    assert_eq!(request.request_span, plan.call_span);
    assert_eq!(request.suggested_grant, "app.charge_card");
    Ok(())
}

/// Invoking an admitted operation preserves the runtime requirements published by its descriptor.
#[test]
fn invoking_a_provider_operation_preserves_its_published_runtime_requirements_on_the_caller()
-> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "run")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("the calling body must lower"))?;

    assert!(
        caller.runtime_requirements.is_empty(),
        "the fixture publishes no runtime requirement, so lowering must not invent one: {:?}",
        caller.runtime_requirements
    );
    Ok(())
}

/// A call the catalog does not admit stays an ordinary named call: nothing may become a plan by spelling alone.
#[test]
fn an_operation_the_catalog_does_not_admit_stays_an_ordinary_call() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(PROVIDER_FIXTURE_SOURCE, &["app"])?;

    assert!(
        provider_plans(&module, "run").is_empty(),
        "an empty catalog must admit nothing"
    );
    assert_eq!(
        named_targets(&module, "run").len(),
        1,
        "the call must still lower normally"
    );
    assert!(refusal_descriptions(&module, "run").is_empty());
    Ok(())
}

/// A disabled provider stops the operation at its source span, with no plan and so nothing to execute.
#[test]
fn an_operation_whose_provider_is_disabled_refuses_before_execution() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_with_provider_operation(
        PROVIDER_FIXTURE_SOURCE,
        &["app"],
        bir::ProviderActivationState::Disabled,
    )?;

    assert!(
        provider_plans(&module, "run").is_empty(),
        "a refused operation must produce no plan, and so no execution and no receipt"
    );
    let refusals = refusal_descriptions(&module, "run");
    assert!(
        refusals.iter().any(|description| description.contains("not enabled")),
        "the refusal must name why the operation cannot run: {refusals:?}"
    );
    Ok(())
}

/// Provider admission is decided before lowering an argument expression. Otherwise a disabled operation could
/// still run source-observable work merely while constructing an execution plan that will never exist.
#[test]
fn a_disabled_provider_refuses_at_the_call_span_before_lowering_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int) -> int:
  return amount

def side_effect() -> str:
  return "acct-1"

def run() -> int:
  return charge(side_effect(), 250)
"#;
    let module = build_with_provider_operation(source, &["app"], bir::ProviderActivationState::Disabled)?;
    let body = body_named(&module, "run")?;
    let refusal = body
        .block
        .stmts
        .iter()
        .find(|statement| matches!(statement.kind, bir::StatementKind::Unsupported { .. }))
        .ok_or("disabled provider call must lower to an explicit refusal")?;
    let expected_start = source.find("charge(side_effect()").ok_or("fixture call missing")?;
    assert_eq!(
        refusal.span.start, expected_start,
        "the refusal must keep the provider call's source span rather than an argument span"
    );
    assert!(
        named_targets(&module, "run")
            .iter()
            .all(|target| target.name != "side_effect"),
        "argument expressions must not be lowered after provider admission refused: {}",
        body.render_snapshot()
    );
    assert!(provider_plans(&module, "run").is_empty());
    Ok(())
}

/// An unavailable provider is refused separately from a disabled one, because the two have different remedies.
#[test]
fn an_operation_whose_provider_is_unavailable_refuses_with_its_own_reason() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_with_provider_operation(
        PROVIDER_FIXTURE_SOURCE,
        &["app"],
        bir::ProviderActivationState::Unavailable,
    )?;

    assert!(provider_plans(&module, "run").is_empty());
    let refusals = refusal_descriptions(&module, "run");
    assert!(
        refusals
            .iter()
            .any(|description| description.contains("no locally available artifact")),
        "an unavailable provider must not be reported as a disabled one: {refusals:?}"
    );
    Ok(())
}

/// Corrupt provider metadata whose authority is not a capability is rejected before lowering can mint a plan.
#[test]
fn corrupt_provider_metadata_with_a_non_capability_authority_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(PROVIDER_FIXTURE_SOURCE).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut provider_plan =
        provider_plan_from_checked_source(checker.type_info(), bir::ProviderActivationState::Active)?;
    let provider = provider_plan
        .records()
        .next()
        .ok_or("fixture provider plan must have one record")?;
    let mut manifest = provider
        .manifest
        .as_deref()
        .cloned()
        .ok_or("fixture provider must have a manifest")?;
    manifest.contract_metadata.provider.operation_descriptors[0]
        .required_capability
        .kind = SemanticSourceTargetKind::Function;

    use std::collections::BTreeSet;
    use std::sync::Arc;

    use crate::library_manifest_index::LibraryManifestIndex;
    use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

    provider_plan = ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "fixture_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "fixture:provider-operation".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Compiler,
            authority: NamespaceAuthority::Compiler,
            namespace_claims: BTreeSet::from([vec!["app".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        [vec!["app".to_string()]],
    )?;
    let error = ProviderOperationCatalog::from_provider_plan(&provider_plan)
        .expect_err("corrupt metadata must not create a lowering catalog");
    assert!(
        error.contains("unsupported declaration kinds"),
        "unexpected error: {error}"
    );
    Ok(())
}

/// A selected provider cannot publish one callable identity twice. Refusing at plan projection makes the result
/// independent of manifest traversal order and prevents a Body-IR call from silently choosing one provider record.
#[test]
fn duplicate_provider_operation_identities_refuse_before_body_ir_building() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::library_manifest_index::LibraryManifestIndex;

    let tokens = lexer::lex(PROVIDER_FIXTURE_SOURCE).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let provider_plan = provider_plan_from_checked_source(checker.type_info(), bir::ProviderActivationState::Active)?;
    let mut provider = provider_plan
        .records()
        .next()
        .cloned()
        .ok_or("fixture provider plan must have one provider")?;
    let manifest = Arc::make_mut(
        provider
            .manifest
            .as_mut()
            .ok_or("fixture provider must have a manifest")?,
    );
    let duplicate = manifest.contract_metadata.provider.operation_descriptors[0].clone();
    manifest
        .contract_metadata
        .provider
        .operation_descriptors
        .push(duplicate);
    let namespace_claims = provider.namespace_claims.clone();
    let colliding_plan = ProviderPlan::new(LibraryManifestIndex::default(), vec![provider], namespace_claims)?;

    let error =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &colliding_plan)
            .expect_err("a duplicate canonical operation identity must not pick one manifest entry");
    assert!(
        error.contains("duplicate provider operation identity `charge`"),
        "unexpected collision refusal: {error}"
    );
    Ok(())
}

/// An omitted parameter has no evaluated input, so the plan cannot honestly describe what would execute.
#[test]
fn an_operation_called_with_an_omitted_default_refuses() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int = 1) -> int:
  return amount

def run() -> int:
  return charge("acct-1")
"#;
    let module = build_with_provider_operation(source, &["app"], bir::ProviderActivationState::Active)?;

    assert!(provider_plans(&module, "run").is_empty());
    let refusals = refusal_descriptions(&module, "run");
    assert!(
        refusals
            .iter()
            .any(|description| description.contains("omitted parameter")),
        "unexpected refusal: {refusals:?}"
    );
    Ok(())
}

/// A callee the typechecker never resolved has no identity, so it cannot become a plan by sharing a spelling.
///
/// The catalog holds a real entry throughout, and the call site writes a name the module does not declare. Nothing
/// about the two spellings is allowed to bring them together: admission runs on canonical identity, and an
/// unresolved callee has none.
///
/// The refusal here is genuinely *source-owned*: the typechecker rejects the program before lowering runs. Body IR
/// deliberately does not raise a second diagnostic for the same gap — see
/// [`BodyBuilder::declared_slots_for_direct_call`], which keeps the ordinary named-call representation with no proven
/// identity so the executor refuses it at the original call span. What this pins is that no plan is minted, so nothing
/// exists that could execute or report.
#[test]
fn an_unresolved_operation_refuses_at_its_source_span_and_produces_no_plan() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int) -> int:
  return amount

def run() -> int:
  return charge_now("acct-1", 250)
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    let diagnostics = checker
        .check_program(&program)
        .err()
        .ok_or("the fixture calls an undeclared name and must be rejected by the source checker")?;
    assert!(!diagnostics.is_empty(), "the refusal must be source-owned first");

    let provider_plan = provider_plan_from_checked_source(checker.type_info(), bir::ProviderActivationState::Active)?;
    let module =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)?;

    assert!(
        provider_plans(&module, "run").is_empty(),
        "an unresolved callee must never be admitted as an operation the catalog holds under another identity"
    );
    let targets = named_targets(&module, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    assert!(
        target.canonical.is_none() && target.direct_call_id.is_none(),
        "an unresolved callee must carry no identity for anything downstream to admit it by"
    );
    Ok(())
}
