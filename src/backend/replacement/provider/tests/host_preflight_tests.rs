//! Same-module provider-host preflight regressions (#1249 F4).
//!
//! The parent fixture supplies the checked provider-plan projection and canonical ledger host. These cases only
//! vary source shapes that can defer a checked operation beyond the selected entry body's top-level statements.

use std::rc::Rc;

use incan_semantics_core::HirSourceSpan;

use super::*;
use crate::backend::replacement::{
    ProgramIo, ReplacementExecutionError, ReplacementValue, execute_prevalidated_free_function_with_io,
    prepare_free_function_execution_with_providers,
};

/// Combine one checked provider declaration with the source-local computation a regression exercises.
fn provider_source(declarations: &str) -> String {
    format!(
        r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

{declarations}
"#
    )
}

/// A malformed caller must retain its own binding diagnostic rather than preflight the callee's provider.
#[test]
fn malformed_named_binding_refuses_at_the_caller_before_provider_discovery() -> TestResult {
    let mut fixture = lower_fixture_source(SIBLING_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let expected_span = unique_source_span(SIBLING_PROVIDER_SOURCE, "debit(account, amount)")?;
    let entry = fixture
        .module
        .bodies
        .iter_mut()
        .find(|body| body.name == "settle")
        .ok_or("fixture must contain settle")?;
    let target = entry
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if statement.span == expected_span => Some(target),
            _ => None,
        })
        .ok_or("fixture must contain the named debit call")?;
    target.binding = bir::ArgumentBinding::UnresolvedPositional;
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];
    let error = prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, None)
        .err()
        .ok_or("malformed call binding must refuse")?;
    assert_eq!(error.primary_span(), Some(expected_span));
    assert!(error.to_string().contains("call to function `debit`"), "{error}");
    assert!(error.operation_receipt().is_none());
    Ok(())
}

/// A hosted deferred computation is inspected without invocation, then executes through the canonical ledger.
fn assert_hosted_deferred(source: &str, args: &[ReplacementValue], expected_stdout: &[u8]) -> TestResult {
    let fixture = lower_fixture_source(source, ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, Some(&fixture.capability), host.clone());
    let prepared = prepare_free_function_execution_with_providers(&fixture.module, "settle", args, Some(&providers))?;
    assert!(host.invocations().is_empty());
    assert!(providers.operation_receipts().is_empty());
    assert!(providers.lifecycle_evidence().is_empty());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let execution = execute_prevalidated_free_function_with_io(prepared, &mut io)?;
    assert_eq!(execution.value, ReplacementValue::Int(255));
    assert_eq!(io.output().stdout(), expected_stdout);
    assert!(io.output().stderr().is_empty());
    assert_eq!(host.invocations(), vec![250]);
    assert_eq!(host.releases(), 1);
    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err("expected exactly one hosted operation receipt".into());
    };
    assert_eq!(receipt.operation(), &fixture.operation);
    assert_eq!(receipt.status(), ReceiptStatus::Allowed);
    Ok(())
}

/// Source defaults, generators and captured closures retain their ordinary hosted behavior.
#[test]
fn deferred_computations_execute_with_their_matching_host() -> TestResult {
    let account = ReplacementValue::Str("acct-1".to_string());
    assert_hosted_deferred(
        SOURCE_DEFAULT_PROVIDER_SOURCE,
        std::slice::from_ref(&account),
        b"before source default provider host\n",
    )?;
    assert_hosted_deferred(
        GENERATOR_PROVIDER_SOURCE,
        &[account.clone(), ReplacementValue::Int(250)],
        b"before generator provider host\n",
    )?;
    assert_hosted_deferred(
        STORED_CLOSURE_PROVIDER_FIXTURE_SOURCE,
        &[account, ReplacementValue::Int(250)],
        b"before provider host\n",
    )
}

/// Generator functions, adapter callbacks and partial defaults participate in preflight even when stored.
#[test]
fn nested_and_lazy_provider_computations_refuse_before_preparation_succeeds() -> TestResult {
    let cases = [
        r#"
def pending() -> Generator[int]:
  yield charge("acct-1", 250)

def settle() -> int:
  values = pending().collect()
  return values[0]
"#,
        r#"
def settle() -> int:
  debit: (int) -> int = (amount) => charge("acct-1", 250)
  values = (value for value in range(0, 1)).map(debit).collect()
  return values[0]
"#,
        r#"
def debit(prefix: int, amount: int = charge("acct-1", 250)) -> int:
  return prefix + amount

def settle() -> int:
  invoke = partial debit(prefix=0)
  return invoke()
"#,
    ];
    for declarations in cases {
        let source = provider_source(declarations);
        let fixture = lower_fixture_source(&source, ProviderActivationState::Active)?;
        let span = unique_source_span(&source, "charge(\"acct-1\", 250)")?;
        assert_missing_host_preflight(&fixture, "settle", &[], span, b"")?;
        assert_hosted_deferred(&source, &[], b"")?;
    }
    Ok(())
}

/// Preflight follows canonical recursive edges once while still discovering a provider after the cycle.
#[test]
fn recursive_preflight_terminates_without_skipping_the_provider() -> TestResult {
    let source = provider_source(
        r#"
def debit(remaining: int) -> int:
  if remaining > 0:
    return relay(remaining - 1)
  return charge("acct-1", 250)

def relay(remaining: int) -> int:
  return debit(remaining)

def settle() -> int:
  return debit(2)
"#,
    );
    let fixture = lower_fixture_source(&source, ProviderActivationState::Active)?;
    let span = unique_source_span(&source, "charge(\"acct-1\", 250)")?;
    assert_missing_host_preflight(&fixture, "settle", &[], span, b"")?;
    assert_hosted_deferred(&source, &[], b"")
}

/// Unrelated provider declarations do not make an otherwise executable entry require a host.
#[test]
fn unreachable_provider_function_is_not_preflighted() -> TestResult {
    let source = format!("{SIBLING_PROVIDER_SOURCE}\ndef quiet() -> int:\n  return 42\n");
    let fixture = lower_fixture_source(&source, ProviderActivationState::Active)?;
    let execution = execute_free_function(&fixture.module, "quiet", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(execution.provider_execution_evidence().is_empty());
    Ok(())
}

/// A matching host never bypasses invocation-time authority, including inside a stored closure.
#[test]
fn nested_hosted_operation_still_requires_invocation_authority() -> TestResult {
    let fixture = lower_fixture_source(STORED_CLOSURE_PROVIDER_FIXTURE_SOURCE, ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, None, host.clone());
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];
    let prepared = prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, Some(&providers))?;
    assert!(providers.operation_receipts().is_empty());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let error = execute_prevalidated_free_function_with_io(prepared, &mut io)
        .err()
        .ok_or("ungranted nested provider operation must refuse")?;
    assert!(matches!(
        error,
        ReplacementExecutionError::ProviderAuthorityDenied { .. }
    ));
    assert_eq!(error.primary_span(), Some(fixture.stored_closure_plan()?.call_span));
    assert_eq!(io.output().stdout(), b"before provider host\n");
    assert!(io.output().stderr().is_empty());
    assert!(host.invocations().is_empty());
    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err("expected one denial receipt".into());
    };
    assert_eq!(receipt.status(), ReceiptStatus::Denied);
    assert_eq!(receipt.operation(), &fixture.operation);
    Ok(())
}

/// A spread call cannot lend its callee's provider span to its own unsupported-argument diagnostic.
#[test]
fn named_spread_refuses_at_the_caller_before_provider_discovery() -> TestResult {
    let mut fixture = lower_fixture_source(SIBLING_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let expected_span = unique_source_span(SIBLING_PROVIDER_SOURCE, "debit(account, amount)")?;
    let entry = fixture
        .module
        .bodies
        .iter_mut()
        .find(|body| body.name == "settle")
        .ok_or("fixture must contain settle")?;
    let arguments = entry
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            bir::StatementKind::Call { args, .. } if statement.span == expected_span => Some(args),
            _ => None,
        })
        .ok_or("fixture must contain the debit call")?;
    let first = arguments.first_mut().ok_or("debit call must supply account")?;
    let source = first
        .as_one()
        .ok_or("account must initially be a fixed operand")?
        .clone();
    *first = bir::ArgumentElement::Spread(bir::SpreadElement {
        source,
        kind: bir::SpreadKind::Sequence,
    });
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];
    let error = prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, None)
        .err()
        .ok_or("spread call must refuse")?;
    assert_eq!(error.primary_span(), Some(expected_span));
    assert!(error.to_string().contains("with a spread argument"), "{error}");
    assert!(error.operation_receipt().is_none());
    Ok(())
}

/// Conservative inspection must not run a supplied-away default, unused closure, or unpolled generator.
#[test]
fn conservative_preflight_does_not_execute_dormant_computations() -> TestResult {
    let cases = [
        (
            r#"
def settle(amount: int = charge("acct-1", 250)) -> int:
  return amount
"#,
            vec![ReplacementValue::Int(42)],
        ),
        (
            r#"
def settle() -> int:
  unused: () -> int = () => charge("acct-1", 250)
  return 42
"#,
            vec![],
        ),
        (
            r#"
def settle() -> int:
  unused = (charge("acct-1", 250) for item in range(0, 1))
  return 42
"#,
            vec![],
        ),
    ];
    for (declarations, args) in cases {
        let source = provider_source(declarations);
        let fixture = lower_fixture_source(&source, ProviderActivationState::Active)?;
        let span = unique_source_span(&source, "charge(\"acct-1\", 250)")?;
        assert_missing_host_preflight(&fixture, "settle", &args, span, b"")?;
        let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
        let providers = runtime(AuthorityMode::Governed, None, host.clone());
        let prepared =
            prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, Some(&providers))?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let execution = execute_prevalidated_free_function_with_io(prepared, &mut io)?;
        assert_eq!(execution.value, ReplacementValue::Int(42));
        assert!(host.invocations().is_empty());
        assert!(providers.operation_receipts().is_empty());
        assert!(io.output().stdout().is_empty());
        assert!(io.output().stderr().is_empty());
    }
    Ok(())
}

const SIBLING_PROVIDER_SOURCE: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def debit(account: str, amount: int) -> int:
  return charge(account, amount)

def settle(account: str, amount: int) -> int:
  println("before sibling provider host")
  return debit(account, amount)
"#;

const SOURCE_DEFAULT_PROVIDER_SOURCE: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def debit(account: str, amount: int = charge("default-account", 250)) -> int:
  return amount

def settle(account: str) -> int:
  println("before source default provider host")
  return debit(account)
"#;

const GENERATOR_PROVIDER_SOURCE: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def settle(account: str, amount: int) -> int:
  println("before generator provider host")
  values = (charge(account, amount) for marker in range(0, 1)).collect()
  return values[0]
"#;

/// Build the exact source span of the one provider call in a fixture.
///
/// The source declaration is lowered through the parent fixture's checked provider-plan projection. Keeping this
/// assertion source-shaped verifies that a preflight failure points at the authored call rather than a generated
/// helper or enclosing declaration, without duplicating the executor's Body-IR traversal in test code.
fn unique_source_span(source: &str, call: &str) -> Result<HirSourceSpan, String> {
    let mut calls = source.match_indices(call);
    let (start, _) = calls
        .next()
        .ok_or_else(|| format!("fixture must contain provider call {call:?}"))?;
    if calls.next().is_some() {
        return Err(format!("fixture must contain exactly one provider call {call:?}"));
    }
    Ok(HirSourceSpan::new(start, start + call.len()))
}

/// Assert that preparation, rather than a partially executed program, rejects an unhosted checked operation.
///
/// Unexpected admission executes with capture writers to report any preceding output and expose a late refusal.
/// Correct preparation always rejects before reaching that branch.
fn assert_missing_host_preflight(
    fixture: &LoweredFixture,
    entry: &str,
    args: &[ReplacementValue],
    expected_span: HirSourceSpan,
    late_stdout: &[u8],
) -> TestResult {
    match prepare_free_function_execution_with_providers(&fixture.module, entry, args, None) {
        Err(error) => {
            let ReplacementExecutionError::Unsupported { description, .. } = &error else {
                return Err(
                    format!("an unhosted provider operation must refuse during preparation, got {error:?}").into(),
                );
            };
            if !description.contains("provider operation `charge`")
                || !description.contains("no provider host in this run")
            {
                return Err(format!(
                    "preflight must name the checked unhosted provider operation, got {description:?}"
                )
                .into());
            }
            if error.primary_span() != Some(expected_span) {
                return Err(format!(
                    "preflight must retain provider call span {expected_span:?}, got {:?}",
                    error.primary_span()
                )
                .into());
            }
            if error.operation_receipt().is_some() {
                return Err("a preflight refusal must not name an operation receipt".into());
            }
            Ok(())
        }
        Ok(prepared) => {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut io = ProgramIo::new(&mut stdout, &mut stderr);
            let error = execute_prevalidated_free_function_with_io(prepared, &mut io)
                .err()
                .ok_or("an unhosted provider operation must not complete after preparation")?;
            if error.operation_receipt().is_some() {
                return Err("a late unhosted refusal must not name an operation receipt".into());
            }
            if io.output().stdout() != late_stdout || !io.output().stderr().is_empty() {
                return Err(format!(
                    "unexpected preparation success retained the wrong output; stdout={:?}, stderr={:?}",
                    io.output().stdout(),
                    io.output().stderr()
                )
                .into());
            }
            Err(format!(
                "provider-host preflight admitted a deferred operation at {expected_span:?}; execution then refused after stdout={:?}",
                io.output().stdout()
            )
            .into())
        }
    }
}

/// A same-module sibling reached after output is part of the selected entry's preflight closure.
#[test]
fn unhosted_provider_operation_in_reachable_sibling_refuses_during_preparation() -> TestResult {
    let fixture = lower_fixture_source(SIBLING_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let expected_span = unique_source_span(SIBLING_PROVIDER_SOURCE, "charge(account, amount)")?;
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];

    assert_missing_host_preflight(
        &fixture,
        "settle",
        &args,
        expected_span,
        b"before sibling provider host\n",
    )
}

/// A matching host admits the same sibling route and proves service dispatch did not execute `charge`'s local stub.
#[test]
fn reachable_sibling_provider_operation_uses_the_matching_host() -> TestResult {
    let fixture = lower_fixture_source(SIBLING_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, Some(&fixture.capability), host.clone());
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];
    let prepared = prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, Some(&providers))?;
    assert_eq!(
        host.descriptions(),
        1,
        "preflight must inspect the reachable operation once"
    );
    assert!(host.invocations().is_empty(), "preparation must not invoke the host");
    assert!(providers.operation_receipts().is_empty());
    assert!(providers.provider_executions().is_empty());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let execution = execute_prevalidated_free_function_with_io(prepared, &mut io)?;

    if execution.value != ReplacementValue::Int(255) {
        return Err(format!(
            "the matching ledger host must add its fixed fee instead of running the local stub, got {:?}",
            execution.value
        )
        .into());
    }
    if io.output().stdout() != b"before sibling provider host\n" || !io.output().stderr().is_empty() {
        return Err(format!(
            "the hosted sibling route must retain only its authored stdout; stdout={:?}, stderr={:?}",
            io.output().stdout(),
            io.output().stderr()
        )
        .into());
    }
    if host.invocations() != vec![250] {
        return Err(format!(
            "the matching host must receive the charge once, got {:?}",
            host.invocations()
        )
        .into());
    }
    if providers.operation_receipts().len() != 1 || providers.provider_executions().len() != 1 {
        return Err("a hosted provider operation must retain one provider execution and operation receipt".into());
    }
    Ok(())
}

/// A nonmatching host rejects the same canonical operation before the sibling's preceding output can run.
#[test]
fn reachable_sibling_provider_operation_with_mismatched_host_refuses_during_preparation() -> TestResult {
    let fixture = lower_fixture_source(SIBLING_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let expected_span = unique_source_span(SIBLING_PROVIDER_SOURCE, "charge(account, amount)")?;
    let host = Rc::new(LedgerHost::new(
        ledger_capability(SemanticSourceTargetKind::Function),
        LedgerBehavior::Settle,
    ));
    let providers = runtime(AuthorityMode::Governed, Some(&fixture.capability), host.clone());
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];

    match prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, Some(&providers)) {
        Err(error) => {
            if error.primary_span() != Some(expected_span) {
                return Err(format!(
                    "a mismatched host must refuse at {expected_span:?}, got {:?}",
                    error.primary_span()
                )
                .into());
            }
            if error.operation_receipt().is_some() || !host.invocations().is_empty() {
                return Err("a mismatched-host preflight refusal must not invoke or receipt an operation".into());
            }
            if !providers.operation_receipts().is_empty() || !providers.provider_executions().is_empty() {
                return Err("a mismatched-host preflight refusal must retain no provider execution evidence".into());
            }
            Ok(())
        }
        Ok(prepared) => {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut io = ProgramIo::new(&mut stdout, &mut stderr);
            let error = execute_prevalidated_free_function_with_io(prepared, &mut io)
                .err()
                .ok_or("a mismatched host must not complete after preparation")?;
            if error.operation_receipt().is_some()
                || !host.invocations().is_empty()
                || !providers.operation_receipts().is_empty()
                || !providers.provider_executions().is_empty()
            {
                return Err("a late mismatched-host refusal must not invoke or receipt an operation".into());
            }
            if io.output().stdout() != b"before sibling provider host\n" || !io.output().stderr().is_empty() {
                return Err(format!(
                    "unexpected preparation success retained the wrong output; stdout={:?}, stderr={:?}",
                    io.output().stdout(),
                    io.output().stderr()
                )
                .into());
            }
            Err(format!(
                "provider-host preflight admitted a mismatched sibling host at {expected_span:?}; execution then refused after stdout={:?}",
                io.output().stdout()
            )
            .into())
        }
    }
}

/// An omitted source default is checked before its reachable callee can execute its preceding output.
#[test]
fn unhosted_provider_operation_in_source_default_refuses_during_preparation() -> TestResult {
    let fixture = lower_fixture_source(SOURCE_DEFAULT_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let expected_span = unique_source_span(SOURCE_DEFAULT_PROVIDER_SOURCE, "charge(\"default-account\", 250)")?;
    let args = [ReplacementValue::Str("acct-1".to_string())];

    assert_missing_host_preflight(
        &fixture,
        "settle",
        &args,
        expected_span,
        b"before source default provider host\n",
    )
}

/// A generator expression's deferred operation is still in the selected entry's preflight closure.
#[test]
fn unhosted_provider_operation_in_generator_expression_refuses_during_preparation() -> TestResult {
    let fixture = lower_fixture_source(GENERATOR_PROVIDER_SOURCE, ProviderActivationState::Active)?;
    let expected_span = unique_source_span(GENERATOR_PROVIDER_SOURCE, "charge(account, amount)")?;
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];

    assert_missing_host_preflight(
        &fixture,
        "settle",
        &args,
        expected_span,
        b"before generator provider host\n",
    )
}
