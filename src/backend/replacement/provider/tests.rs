//! Direct execution of the selected first provider-service operation, one path per test (#1156).
//!
//! Every fixture here goes through the real pipeline — source, typecheck, Body-IR lowering with a
//! fixture-controlled provider catalog — so what the executor consumes is an actually-lowered
//! [`ProviderOperationPlan`], not a hand-assembled one. The defensive activation, binding and spread tests deliberately
//! mutate already-lowered facts to prove the runtime rejects malformed input without changing the diagnostic's owner.
//!
//! The fixture host keys on the operation's [`CanonicalSymbolId`] and on nothing else. That is not a stylistic
//! choice: a host that matched a provider module name, a call-site spelling, or an emitted Rust name would be the
//! exact duplication of source meaning this vertical exists to avoid, and a test that let one through would stop
//! being evidence.

use std::cell::RefCell;

mod host_preflight_tests;

use incan_semantics_core::body_ir::{self as bir, ProviderActivationState, ProviderOperationPlan};
use incan_semantics_core::receipts::{AttributeSensitivity, ReceiptStatus, ReplayClassification};
use incan_semantics_core::{
    AuthorityDenialReason, AuthorityMode, CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolOrigin,
    authority::StaticAuthority,
};

use super::*;
use crate::backend::replacement::{
    ProgramIo, ReplacementExecution, ReplacementExecutionError, ReplacementValue, execute_free_function,
    execute_free_function_with_providers, execute_prevalidated_free_function_with_io,
    prepare_free_function_execution_with_providers,
};
use crate::frontend::body_ir::build_body_ir_module_v0_with_provider_plan;
use crate::frontend::body_ir::tests::provider_plan_from_checked_source;
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{ast, lexer, parser};

/// A boxed error, so every fallible test can propagate with `?` instead of unwrapping.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The module the fixture operation is declared in.
const MODULE_PATH: &[&str] = &["app"];

/// The grant spelling the selected capability renders to, and therefore the one a governed run must hold.
///
/// It is derived from the fixture's own `capability ledger_charge` declaration rather than chosen here, because the
/// decorator resolves the capability the provider manifest publishes. A grant spelling invented by the test would
/// prove the executor honours a string no declaration produces.
const LEDGER_GRANT: &str = "app.ledger_charge";

/// One ledger charge, plus a same-module caller that invokes it.
///
/// `charge`'s own body deliberately returns a *different* value than the provider host does. Nothing should ever
/// execute it: the plan names a provider operation, and running the local declaration instead would silently
/// substitute source-local behavior for the service's. The difference is what makes that substitution visible.
const LEDGER_FIXTURE_SOURCE: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def settle(account: str, amount: int) -> int:
  return charge(account, amount)
"#;

/// One checked provider invocation retained inside a stored closure.
///
/// The outer output makes a late missing-host refusal source-observable. The regression below must make preparation
/// refuse before that `println` can execute; unexpected admission is exercised with capture writers to expose
/// any output before the late refusal.
const STORED_CLOSURE_PROVIDER_FIXTURE_SOURCE: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def settle(account: str, amount: int) -> int:
  println("before provider host")
  invoke: (str, int) -> int = (captured_account, captured_amount) => charge(captured_account, captured_amount)
  return invoke(account, amount)
"#;

/// What the fixture ledger does when an authorized charge reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LedgerBehavior {
    /// Settle the charge, adding a fixed fee so the result cannot be confused with the local declaration's.
    Settle,
    /// Settle the charge but withhold the account identifier from the receipt.
    SettleWithSecretAccount,
    /// Refuse the charge after authority was already granted.
    Decline,
}

/// Everything the fixture ledger observed, so a test can assert on what did and did not happen.
#[derive(Debug, Default)]
struct LedgerLog {
    /// One entry per [`ProviderOperationHost::invoke`] call, holding the amount it was asked to charge.
    invocations: Vec<i64>,
    /// How many settlement handles are currently open.
    open_handles: i64,
    /// How many times [`ProviderOperationHost::release`] ran.
    releases: usize,
    /// How many times this host was asked to describe an operation.
    descriptions: usize,
}

/// A fixture ledger provider, addressed only by the canonical identity of the operation it owns.
#[derive(Debug)]
struct LedgerHost {
    operation: CanonicalSymbolId,
    behavior: LedgerBehavior,
    log: RefCell<LedgerLog>,
}

impl LedgerHost {
    /// Build a host that executes exactly `operation` and behaves as `behavior` when it is invoked.
    fn new(operation: CanonicalSymbolId, behavior: LedgerBehavior) -> Self {
        Self {
            operation,
            behavior,
            log: RefCell::new(LedgerLog::default()),
        }
    }

    /// The amounts this host was actually asked to charge, in invocation order.
    fn invocations(&self) -> Vec<i64> {
        self.log.borrow().invocations.clone()
    }

    /// How many settlement handles this host still holds open.
    fn open_handles(&self) -> i64 {
        self.log.borrow().open_handles
    }

    /// How many times this host released a settlement handle.
    fn releases(&self) -> usize {
        self.log.borrow().releases
    }

    /// How many times this host was asked to describe the operation without being asked to perform it.
    fn descriptions(&self) -> usize {
        self.log.borrow().descriptions
    }

    /// The integer amount carried by the input at written position 1, or an error naming what arrived instead.
    fn amount(inputs: &[ProviderInputValue]) -> Result<i64, String> {
        match inputs.iter().find(|input| input.written_position == 1) {
            Some(ProviderInputValue {
                value: ReplacementValue::Int(amount),
                ..
            }) => Ok(*amount),
            other => Err(format!("a charge needs an integer amount, got {other:?}")),
        }
    }
}

impl ProviderOperationHost for LedgerHost {
    fn operation_kind(&self, operation: &CanonicalSymbolId) -> Option<String> {
        self.log.borrow_mut().descriptions += 1;
        (operation == &self.operation).then(|| "ledger.charge".to_string())
    }

    fn invoke(&self, invocation: &ProviderInvocation<'_, '_>) -> ProviderOperationOutcome {
        let amount = match LedgerHost::amount(invocation.inputs) {
            Ok(amount) => amount,
            Err(detail) => {
                return ProviderOperationOutcome::Failed {
                    detail,
                    attributes: Vec::new(),
                    replay: ReplayClassification::Unavailable,
                };
            }
        };
        {
            let mut log = self.log.borrow_mut();
            log.invocations.push(amount);
            log.open_handles += 1;
        }
        match self.behavior {
            LedgerBehavior::Settle => ProviderOperationOutcome::Completed {
                value: ReplacementValue::Int(amount + 5),
                attributes: vec![ReceiptAttribute::public("ledger.amount", amount.to_string())],
                replay: ReplayClassification::FixtureRequired,
            },
            LedgerBehavior::SettleWithSecretAccount => ProviderOperationOutcome::Completed {
                value: ReplacementValue::Int(amount + 5),
                attributes: vec![
                    ReceiptAttribute::public("ledger.amount", amount.to_string()),
                    ReceiptAttribute::redacted("ledger.account", AttributeSensitivity::Secret),
                ],
                replay: ReplayClassification::FixtureRequired,
            },
            LedgerBehavior::Decline => ProviderOperationOutcome::Failed {
                detail: format!("the ledger declined a charge of {amount}"),
                attributes: vec![ReceiptAttribute::public("ledger.amount", amount.to_string())],
                replay: ReplayClassification::FixtureRequired,
            },
        }
    }

    fn release(&self, _operation: &CanonicalSymbolId, _call_span: HirSourceSpan) {
        let mut log = self.log.borrow_mut();
        log.open_handles -= 1;
        log.releases += 1;
    }
}

/// Canonical identity of the same-module `def` named `name`, minted exactly as lowering mints it.
///
/// The spelling is only how this fixture *finds* the declaration whose identity becomes the catalog key; nothing
/// downstream ever sees it.
fn local_function_identity(program: &ast::Program, name: &str) -> Option<CanonicalSymbolId> {
    let module_path: Vec<String> = MODULE_PATH.iter().map(|segment| (*segment).to_string()).collect();
    program.declarations.iter().find_map(|declaration| {
        let ast::Declaration::Function(function) = &declaration.node else {
            return None;
        };
        (function.name == name).then(|| {
            CanonicalSymbolId::module_declaration(
                module_path.clone(),
                name,
                SemanticSourceTargetKind::Function,
                HirSourceSpan::new(declaration.span.start, declaration.span.end),
            )
        })
    })
}

/// Canonical identity of the RFC 104 capability a ledger charge needs.
fn ledger_capability(kind: SemanticSourceTargetKind) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["host".to_string(), "ledger".to_string()],
        "charge",
        kind,
        HirSourceSpan::new(1, 2),
    )
}

/// The lowered fixture module plus the canonical identity of the operation it admits.
struct LoweredFixture {
    module: bir::BodyIrModule,
    operation: CanonicalSymbolId,
}

impl LoweredFixture {
    /// The single provider-operation plan the `settle` body lowered.
    fn plan(&self) -> Result<&ProviderOperationPlan, String> {
        let plans: Vec<&ProviderOperationPlan> = self
            .module
            .bodies
            .iter()
            .filter(|body| body.name == "settle")
            .flat_map(|body| &body.block.stmts)
            .filter_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::ProviderOperation(plan),
                    ..
                } => Some(plan.as_ref()),
                _ => None,
            })
            .collect();
        match plans.as_slice() {
            [plan] => Ok(plan),
            other => Err(format!("expected exactly one lowered plan, got {}", other.len())),
        }
    }

    /// The one admitted provider plan retained in `settle`'s stored closure body.
    fn stored_closure_plan(&self) -> Result<&ProviderOperationPlan, String> {
        let settle = self
            .module
            .bodies
            .iter()
            .find(|body| body.name == "settle")
            .ok_or("fixture must retain a `settle` body")?;
        let mut closures = settle.block.stmts.iter().filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } => Some(body.as_ref()),
            _ => None,
        });
        let closure = closures
            .next()
            .ok_or("fixture must lower the provider call into one stored closure")?;
        if closures.next().is_some() {
            return Err("fixture must lower exactly one stored closure".to_string());
        }
        let plans: Vec<&ProviderOperationPlan> = closure
            .stmts
            .iter()
            .filter_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::ProviderOperation(plan),
                    ..
                } => Some(plan.as_ref()),
                _ => None,
            })
            .collect();
        match plans.as_slice() {
            [plan] => Ok(plan),
            other => Err(format!(
                "expected exactly one provider plan in the stored closure, got {}",
                other.len()
            )),
        }
    }

    /// Replace the lowered plan's provider activation, producing a plan lowering would have refused.
    ///
    /// Used only to prove the runtime is fail-closed rather than trusting the gate upstream of it.
    fn force_activation(&mut self, state: ProviderActivationState) {
        for body in &mut self.module.bodies {
            for statement in &mut body.block.stmts {
                if let bir::StatementKind::Call {
                    callee: bir::Callee::ProviderOperation(plan),
                    ..
                } = &mut statement.kind
                {
                    plan.provider.state = state;
                }
            }
        }
    }
}

/// Lower the ledger fixture with `charge` admitted as a fixture-controlled provider operation.
///
/// The call site is told nothing. Admission travels entirely through the operation's canonical identity, which is
/// why the same fixture proves both that an admitted call reaches the executor and that an unadmitted one does not.
fn lower_fixture(activation: ProviderActivationState) -> Result<LoweredFixture, Box<dyn std::error::Error>> {
    lower_fixture_source(LEDGER_FIXTURE_SOURCE, activation)
}

/// Lower one provider fixture source through the checked provider-plan projection.
fn lower_fixture_source(
    source: &str,
    activation: ProviderActivationState,
) -> Result<LoweredFixture, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path: Vec<String> = MODULE_PATH.iter().map(|segment| (*segment).to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;

    // Admission is projected from the checked provider manifest, never hand-filled here. #1213 made the catalogue
    // private for exactly this reason: a test that registers its own entry would prove the executor works on a
    // catalogue no producer could actually have published, which is the handwritten module exception #1156 exists
    // to rule out. This shares the builder with #1213's own tests so both layers exercise one admission path.
    let provider_plan = provider_plan_from_checked_source(checker.type_info(), activation)?;
    let operation = local_function_identity(&program, "charge").ok_or("the fixture declares no `charge`")?;
    let module =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)?;
    Ok(LoweredFixture { module, operation })
}

/// Build the runtime one test runs against: a governed or permissive authority, and a ledger host.
fn runtime(mode: AuthorityMode, grants: &[&str], host: Rc<LedgerHost>) -> Rc<ProviderRuntime> {
    let authority = StaticAuthority::new(mode, grants.iter().map(|grant| (*grant).to_string()));
    ProviderRuntime::new(Rc::new(authority), host)
}

/// An unhosted operation inside a stored closure must fail at preparation, before preceding output can execute.
///
/// Unexpected admission executes with capture writers so a failure reports the source-observable prefix as well
/// as the late refusal. A correct preflight never enters that branch.
#[test]
fn unhosted_provider_operation_in_stored_closure_refuses_during_preparation() -> TestResult {
    let fixture = lower_fixture_source(STORED_CLOSURE_PROVIDER_FIXTURE_SOURCE, ProviderActivationState::Active)?;
    let plan = fixture.stored_closure_plan()?;
    if plan.operation != fixture.operation {
        return Err("stored closure provider call must retain the fixture's canonical operation identity".into());
    }
    let expected_span = plan.call_span;
    let args = [ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)];

    match prepare_free_function_execution_with_providers(&fixture.module, "settle", &args, None) {
        Err(error) => {
            let ReplacementExecutionError::Unsupported { description, .. } = &error else {
                return Err(format!(
                    "an unhosted stored-closure provider call must refuse during preparation, got {error:?}"
                )
                .into());
            };
            if !description.contains("provider operation `charge`")
                || !description.contains("no provider host in this run")
            {
                return Err(format!(
                    "preparation must name the canonical unhosted provider operation, got {description:?}"
                )
                .into());
            }
            if error.primary_span() != Some(expected_span) {
                return Err(format!(
                    "preparation must refuse at the nested provider call span {expected_span:?}, got {:?}",
                    error.primary_span()
                )
                .into());
            }
            if error.operation_receipt().is_some() {
                return Err("a pre-execution provider-host refusal must not name an operation receipt".into());
            }
            Ok(())
        }
        Ok(prepared) => {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut io = ProgramIo::new(&mut stdout, &mut stderr);
            let error = execute_prevalidated_free_function_with_io(prepared, &mut io)
                .err()
                .ok_or("unexpectedly prepared stored-closure provider call must not complete")?;
            let ReplacementExecutionError::Unsupported { description, .. } = &error else {
                return Err(format!(
                    "late stored-closure provider failure must be an unsupported refusal, got {error:?}"
                )
                .into());
            };
            if !description.contains("provider operation `charge`")
                || !description.contains("without a provider runtime")
            {
                return Err(format!(
                    "late stored-closure provider failure must identify the missing runtime, got {description:?}"
                )
                .into());
            }
            if error.primary_span() != Some(expected_span) {
                return Err(format!(
                    "late provider refusal must retain nested call span {expected_span:?}, got {:?}",
                    error.primary_span()
                )
                .into());
            }
            if error.operation_receipt().is_some() {
                return Err("a missing-provider runtime refusal must not name an operation receipt".into());
            }
            if io.output().stdout() != b"before provider host\n" || !io.output().stderr().is_empty() {
                return Err(format!(
                    "unexpected preparation success must retain only the prior println output; stdout={:?}, stderr={:?}",
                    io.output().stdout(),
                    io.output().stderr()
                )
                .into());
            }
            Err(format!(
                "provider-host preflight admitted a stored closure; execution then refused late at {expected_span:?} after stdout={:?}",
                io.output().stdout()
            )
            .into())
        }
    }
}

/// Execute `settle("acct-1", 250)` against `providers`.
fn settle(
    module: &bir::BodyIrModule,
    providers: &Rc<ProviderRuntime>,
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    execute_free_function_with_providers(
        module,
        "settle",
        &[ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)],
        providers,
    )
}

/// The lifecycle transition labels one runtime recorded, in order.
fn lifecycle_events(providers: &ProviderRuntime) -> Vec<&'static str> {
    providers
        .lifecycle_evidence()
        .into_iter()
        .map(|event| event.event)
        .collect()
}

// ============================================================================
// Allowed invocation
// ============================================================================

/// An allowed invocation runs the provider, not the operation's own local declaration, and its backend execution
/// receipt references the RFC 104 operation receipt rather than restating it.
#[test]
fn an_allowed_provider_operation_executes_and_references_its_operation_receipt() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host.clone());

    let execution = settle(&fixture.module, &providers)?;

    assert_eq!(
        execution.value,
        ReplacementValue::Int(255),
        "the provider settled the charge; the operation's own local body would have returned 250",
    );
    assert_eq!(host.invocations(), vec![250]);

    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err(Box::from(format!(
            "expected one operation receipt, got {}",
            receipts.len()
        )));
    };
    assert_eq!(receipt.status(), ReceiptStatus::Allowed);
    assert_eq!(receipt.operation_kind(), "ledger.charge");
    assert_eq!(
        receipt.capability().origin,
        SymbolOrigin::Module(vec!["app".to_string()]),
        "the receipt names the capability identity the plan carried, not a grant spelling",
    );
    assert_eq!(receipt.operation(), &fixture.operation);
    assert!(receipt.authority().is_allowed());
    receipt.validate().map_err(|violation| violation.to_string())?;

    let evidence = execution.provider_execution_evidence();
    let [backend_receipt] = evidence.as_slice() else {
        return Err(Box::from(format!(
            "expected one backend provider execution, got {}",
            evidence.len()
        )));
    };
    assert_eq!(
        backend_receipt.operation_receipt_sequence_id,
        receipt.sequence_id(),
        "the backend execution receipt must reference the operation receipt rather than copy it",
    );
    assert_eq!(backend_receipt.outcome, "allowed");
    assert!(backend_receipt.selection_identity.starts_with("sha256:"));
    assert!(backend_receipt.receipt_identity.starts_with("sha256:"));
    assert_eq!(
        backend_receipt.comparison_reason, PROVIDER_COMPARISON_UNAVAILABLE_REASON,
        "an executed provider operation must declare an explicitly non-green comparison until #1146 lands",
    );
    Ok(())
}

// Runtime-requirement propagation is deliberately not asserted here, and the reason is upstream rather than a
// gap in this executor. `@provider_operation` currently records `runtime_requirements: Vec::new()`, so a checked
// provider manifest publishes none and every plan carries an empty list. A test could only pass by hand-injecting
// a requirement into the catalogue -- the handwritten exception #1213's rework closed off -- or by asserting an
// empty list flows through to an empty list, which asserts nothing. Neither is worth shipping. Once the
// declaration side derives requirements, propagation belongs back here as a real assertion.

/// Mode semantics belong to the authority seam: a permissive run allows a capability no grant set holds, and this
/// backend neither knows nor decides that.
#[test]
fn a_permissive_run_executes_an_ungranted_provider_operation() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Permissive, &[], host.clone());

    let execution = settle(&fixture.module, &providers)?;

    assert_eq!(execution.value, ReplacementValue::Int(255));
    assert_eq!(host.invocations(), vec![250]);
    Ok(())
}

// ============================================================================
// Governed denial
// ============================================================================

/// A governed denial emits a denied receipt, reports a source-owned diagnostic, and never reaches the provider.
///
/// This is the invariant the whole delivery train exists to protect: the only path to
/// [`ProviderOperationHost::invoke`] runs through an allowed decision, so a denial cannot leave the operation
/// half-performed or performed-then-reported.
#[test]
fn a_governed_denial_emits_a_denied_receipt_without_invoking_the_provider() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, &[], host.clone());
    let call_span = fixture.plan()?.call_span;

    let error = settle(&fixture.module, &providers)
        .err()
        .ok_or("a governed run with no grant must refuse the charge")?;

    let ReplacementExecutionError::ProviderAuthorityDenied { operation, reason, .. } = &error else {
        return Err(Box::from(format!("expected an authority denial, got {error:?}")));
    };
    assert_eq!(operation, "charge");
    assert!(
        reason.contains(LEDGER_GRANT),
        "a denial must name the grant that would permit it: {reason}",
    );
    assert_eq!(error.diagnostic_code(), "INCAN-R1156-DENIED");
    assert_eq!(
        error.primary_span(),
        Some(call_span),
        "a denial is reported at the invocation the source wrote",
    );

    assert!(
        host.invocations().is_empty(),
        "a denied operation must never reach the provider",
    );
    assert_eq!(host.releases(), 0, "nothing was acquired, so nothing may be released");
    assert!(
        host.descriptions() > 0,
        "the host was still asked what kind of operation this is, which is a description and not a performance",
    );

    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err(Box::from(format!(
            "a denial is a recorded outcome, not an absent one; got {} receipts",
            receipts.len()
        )));
    };
    assert_eq!(receipt.status(), ReceiptStatus::Denied);
    assert!(receipt.attributes().is_empty(), "nothing ran, so nothing was recorded");
    assert_eq!(receipt.replay(), ReplayClassification::Unavailable);
    assert_eq!(
        receipt.authority().denial_reason(),
        Some(AuthorityDenialReason::NotGranted),
    );
    assert_eq!(
        error.operation_receipt().map(|reference| reference.sequence_id),
        Some(receipt.sequence_id()),
    );
    receipt.validate().map_err(|violation| violation.to_string())?;

    assert_eq!(
        lifecycle_events(&providers),
        vec!["denied"],
        "a denial records only that it was denied; there is no invocation to release",
    );
    Ok(())
}

/// A host ceiling denies a grant the invocation held, and this backend reports it without interpreting the rule.
#[test]
fn a_host_ceiling_denial_is_reported_without_being_reinterpreted() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let authority = StaticAuthority::new(AuthorityMode::Governed, [LEDGER_GRANT.to_string()])
        .with_ceiling(["host.fs.read".to_string()]);
    let providers = ProviderRuntime::new(Rc::new(authority), host.clone());

    let error = settle(&fixture.module, &providers)
        .err()
        .ok_or("a grant outside the host ceiling must be refused")?;

    assert!(matches!(
        error,
        ReplacementExecutionError::ProviderAuthorityDenied { .. }
    ));
    assert!(host.invocations().is_empty());
    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err(Box::from("a ceiling denial still emits its receipt"));
    };
    assert_eq!(
        receipt.authority().denial_reason(),
        Some(AuthorityDenialReason::OutsideCeiling),
        "the denial reason is the authority source's answer, not one this backend derived",
    );
    Ok(())
}

// ============================================================================
// Provider failure and lifecycle cleanup
// ============================================================================

/// A provider failure keeps its allowing authority decision, and releases what the invocation acquired.
#[test]
fn a_provider_failure_keeps_allowed_authority_and_still_releases() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Decline));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host.clone());

    let error = settle(&fixture.module, &providers)
        .err()
        .ok_or("a declined charge must surface as a failure")?;

    let ReplacementExecutionError::ProviderOperationFailed { operation, detail, .. } = &error else {
        return Err(Box::from(format!("expected a provider failure, got {error:?}")));
    };
    assert_eq!(operation, "charge");
    assert!(
        detail.contains("declined"),
        "the provider's own reason survives: {detail}"
    );
    assert_eq!(error.diagnostic_code(), "INCAN-R1156-PROVIDER");

    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err(Box::from("a failed invocation still emits its receipt"));
    };
    assert_eq!(receipt.status(), ReceiptStatus::Failed);
    assert!(
        receipt.authority().is_allowed(),
        "a failure is not a denial: authority was granted and the operation itself failed",
    );
    receipt.validate().map_err(|violation| violation.to_string())?;

    assert_eq!(host.invocations(), vec![250]);
    assert_eq!(host.releases(), 1);
    assert_eq!(
        host.open_handles(),
        0,
        "a failed charge still holds the settlement handle it opened until cleanup runs",
    );
    assert_eq!(lifecycle_events(&providers), vec!["invoked", "failed", "released"]);
    Ok(())
}

/// A successful invocation releases exactly once, after it completed and before execution continues.
#[test]
fn a_completed_invocation_releases_exactly_once_after_completing() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host.clone());

    settle(&fixture.module, &providers)?;

    assert_eq!(host.releases(), 1);
    assert_eq!(host.open_handles(), 0);
    assert_eq!(
        lifecycle_events(&providers),
        vec!["invoked", "completed", "released"],
        "cleanup follows the outcome it is cleaning up after, and never precedes the invocation",
    );
    Ok(())
}

// ============================================================================
// Redaction classification
// ============================================================================

/// A withheld attribute classifies the receipt as redacted, keeps its key and sensitivity, and never leaks a value.
#[test]
fn a_withheld_attribute_classifies_the_receipt_as_redacted() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let host = Rc::new(LedgerHost::new(
        fixture.operation.clone(),
        LedgerBehavior::SettleWithSecretAccount,
    ));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host);

    let execution = settle(&fixture.module, &providers)?;

    assert_eq!(execution.value, ReplacementValue::Int(255));
    let receipts = providers.operation_receipts();
    let [receipt] = receipts.as_slice() else {
        return Err(Box::from("a redacted invocation still emits one receipt"));
    };
    assert_eq!(
        receipt.status(),
        ReceiptStatus::Redacted,
        "a receipt with withheld values must stop claiming it recorded everything",
    );
    assert_eq!(receipt.redacted_keys(), vec!["ledger.account".to_string()]);
    let withheld = receipt
        .attributes()
        .iter()
        .find(|attribute| attribute.key() == "ledger.account")
        .ok_or("the withheld attribute must keep its key")?;
    assert_eq!(withheld.value(), None);
    assert_eq!(withheld.sensitivity(), AttributeSensitivity::Secret);
    assert!(
        !receipt
            .attributes()
            .iter()
            .any(|attribute| { attribute.value().is_some_and(|value| value.contains("acct-1")) }),
        "the account identifier the call passed must not reach the receipt in the clear",
    );
    receipt.validate().map_err(|violation| violation.to_string())?;

    let evidence = execution.provider_execution_evidence();
    assert_eq!(
        evidence.first().map(|record| record.outcome),
        Some("redacted"),
        "the backend execution receipt records the classification rather than re-deriving it later",
    );
    Ok(())
}

// ============================================================================
// Activation and unresolved calls: refuse before execution, emit no receipt
// ============================================================================

/// An inactive provider never produces a plan at all, so there is nothing for an executor to run or receipt.
#[test]
fn a_disabled_provider_is_refused_by_lowering_before_a_plan_exists() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Disabled)?;
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host.clone());

    assert!(
        fixture.plan().is_err(),
        "a disabled provider must not reach a checked execution plan",
    );
    let error = settle(&fixture.module, &providers)
        .err()
        .ok_or("a refused operation cannot be executed")?;

    assert_eq!(error.diagnostic_code(), "INCAN-R988-UNSUPPORTED");
    assert!(
        error.primary_span().is_some(),
        "a refusal keeps its original source span"
    );
    assert!(host.invocations().is_empty());
    assert!(
        providers.operation_receipts().is_empty() && providers.provider_executions().is_empty(),
        "a refusal that happened before execution has no receipt to emit",
    );
    Ok(())
}

/// The runtime is fail-closed about activation: a plan that claims an inactive provider is refused even though
/// lowering would never have produced one.
#[test]
fn an_inactive_plan_is_refused_by_the_runtime_before_anything_executes() -> TestResult {
    let mut fixture = lower_fixture(ProviderActivationState::Active)?;
    fixture.force_activation(ProviderActivationState::Unavailable);
    let host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host.clone());
    let call_span = fixture.plan()?.call_span;

    let error = prepare_free_function_execution_with_providers(
        &fixture.module,
        "settle",
        &[ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)],
        Some(&providers),
    )
    .err()
    .ok_or("an unavailable provider must be refused")?;

    assert_eq!(error.primary_span(), Some(call_span));
    assert!(
        error.to_string().contains("unavailable"),
        "the refusal must say which activation state blocked it: {error}",
    );
    assert!(host.invocations().is_empty());
    assert!(providers.operation_receipts().is_empty());
    Ok(())
}

/// An operation no host in this run executes is refused before execution, at its source span, with no receipt.
#[test]
fn an_unresolved_provider_operation_is_refused_before_execution() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    // A host that owns a different operation identity resolves nothing here, which is exactly the shape of a run
    // whose provider set does not include the one the program invokes.
    let other_operation = ledger_capability(SemanticSourceTargetKind::Function);
    let host = Rc::new(LedgerHost::new(other_operation, LedgerBehavior::Settle));
    let providers = runtime(AuthorityMode::Governed, &[LEDGER_GRANT], host.clone());
    let call_span = fixture.plan()?.call_span;

    let error = prepare_free_function_execution_with_providers(
        &fixture.module,
        "settle",
        &[ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)],
        Some(&providers),
    )
    .err()
    .ok_or("an operation no host executes must be refused")?;

    assert_eq!(error.diagnostic_code(), "INCAN-R988-UNSUPPORTED");
    assert_eq!(error.primary_span(), Some(call_span));
    assert!(error.operation_receipt().is_none());
    assert!(host.invocations().is_empty());
    assert!(
        providers.operation_receipts().is_empty() && providers.provider_executions().is_empty(),
        "a call refused before execution must produce no execution receipt",
    );
    Ok(())
}

/// A run with no provider runtime refuses an admitted provider operation rather than falling back to anything.
#[test]
fn a_run_without_a_provider_runtime_refuses_visibly() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;
    let call_span = fixture.plan()?.call_span;

    let error = execute_free_function(
        &fixture.module,
        "settle",
        &[ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)],
    )
    .err()
    .ok_or("without a provider runtime there is nothing that could execute the operation")?;

    assert_eq!(error.diagnostic_code(), "INCAN-R988-UNSUPPORTED");
    assert_eq!(error.primary_span(), Some(call_span));
    Ok(())
}

// A plan whose required authority does not name a capability is no longer testable from this layer, and that is
// the right outcome rather than a lost case. #1213's rework rejects such provider metadata *before* lowering can
// mint a plan, so the executor can no longer be handed one — see
// `src/frontend/body_ir/tests.rs`'s "corrupt provider metadata whose authority is not a capability is rejected
// before lowering can mint a plan". Reconstructing it here would mean hand-filling a catalogue no producer could
// publish, which is exactly what that rework closed off.

// ============================================================================
// Evidence identity
// ============================================================================

/// Two identical runs agree on their output identity, and a run that redacts differs from one that does not.
///
/// The provider evidence is bound into the execution's output identity, so a consumer cannot read one execution's
/// identity and believe it describes a differently-receipted run.
#[test]
fn provider_evidence_is_bound_into_the_execution_output_identity() -> TestResult {
    let fixture = lower_fixture(ProviderActivationState::Active)?;

    let settled_host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let settled = settle(
        &fixture.module,
        &runtime(AuthorityMode::Governed, &[LEDGER_GRANT], settled_host),
    )?;
    let repeat_host = Rc::new(LedgerHost::new(fixture.operation.clone(), LedgerBehavior::Settle));
    let repeated = settle(
        &fixture.module,
        &runtime(AuthorityMode::Governed, &[LEDGER_GRANT], repeat_host),
    )?;
    assert_eq!(
        settled.output_identity, repeated.output_identity,
        "the same source, arguments, and provider outcome must produce the same identity",
    );

    let redacted_host = Rc::new(LedgerHost::new(
        fixture.operation.clone(),
        LedgerBehavior::SettleWithSecretAccount,
    ));
    let redacted = settle(
        &fixture.module,
        &runtime(AuthorityMode::Governed, &[LEDGER_GRANT], redacted_host),
    )?;
    assert_ne!(
        settled.output_identity, redacted.output_identity,
        "a run whose receipt withheld a value is not the same observation as one that did not",
    );
    Ok(())
}
