//! The #1156 provider-operation paths: a fixture ledger host driven through the real pipeline (typecheck, Body
//! IR lowering with a fixture-controlled provider catalog, direct replacement execution) and the five callbacks
//! that give the allowed, denied, failed, redacted and cleaned-up paths their own stable #987 dispositions.

use super::*;

// The five rows below give the #1156 vertical's paths their own stable #987 dispositions. Each probe drives the
// real pipeline -- source, typecheck, Body-IR lowering with a fixture-controlled provider catalog, direct
// replacement execution -- against a fixture ledger host and a `StaticAuthority`, and asserts what that path is
// contracted to do.
//
// None of them can be comparison-green, and none claims to be. The legacy backend cannot execute a provider
// operation at all, so there is no second route to compare against until #1146 supplies the receipt-bound paired
// comparison; each probe asserts that the execution it observed declared that non-green state explicitly rather
// than leaving it implied.
//
// `ReplacementExecutionPlan` -- the corpus's direct execution shape -- names a function and concrete arguments,
// with nowhere to name the authority source and provider host a provider operation needs. These callbacks therefore
// inspect the real provider receipts internally, while the outer corpus row records only a `BehaviorObserved`
// evidence identity over the callback outcome. It does not fabricate a legacy receipt or borrow the provider's
// nested receipt as if it authorized the whole row. A corpus-visible provider execution receipt belongs with
// #1146's explicit comparison route rather than with this vertical.

/// One ledger charge, plus a same-module caller that invokes it.
///
/// `charge`'s own body returns a different value than the provider host does, so a run that executed the local
/// declaration instead of the provider would be visible in the observable rather than silent.
pub(super) const PROVIDER_CASE_SRC: &str = r#"
capability ledger_charge:
  description = "Charge one approved ledger account"

@provider_operation(ledger_charge)
def charge(account: str, amount: int) -> int:
  return amount

def settle(account: str, amount: int) -> int:
  return charge(account, amount)
"#;

/// What the fixture ledger does when an authorized charge reaches it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LedgerBehavior {
    /// Settle the charge, adding a fixed fee so the result cannot be confused with the local declaration's.
    Settle,
    /// Settle the charge but withhold the account identifier from the receipt.
    SettleWithSecretAccount,
    /// Refuse the charge after authority was already granted.
    Decline,
}

/// A fixture ledger provider, addressed only by the canonical identity of the operation it owns.
///
/// Keying on the identity rather than on a name is the contract, not a detail: a host that matched a provider
/// module name, a call-site spelling, or an emitted Rust name would be the source-meaning duplication this vertical
/// exists to avoid.
struct CorpusLedgerHost {
    operation: CanonicalSymbolId,
    behavior: LedgerBehavior,
    invocations: RefCell<Vec<i64>>,
    releases: Cell<usize>,
}

impl CorpusLedgerHost {
    /// Build a host that executes exactly `operation` and behaves as `behavior` when it is invoked.
    fn new(operation: CanonicalSymbolId, behavior: LedgerBehavior) -> Self {
        Self {
            operation,
            behavior,
            invocations: RefCell::new(Vec::new()),
            releases: Cell::new(0),
        }
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

impl ProviderOperationHost for CorpusLedgerHost {
    fn operation_kind(&self, operation: &CanonicalSymbolId) -> Option<String> {
        (operation == &self.operation).then(|| "ledger.charge".to_string())
    }

    fn invoke(&self, invocation: &ProviderInvocation<'_, '_>) -> ProviderOperationOutcome {
        let amount = match CorpusLedgerHost::amount(invocation.inputs) {
            Ok(amount) => amount,
            Err(detail) => {
                return ProviderOperationOutcome::Failed {
                    detail,
                    attributes: Vec::new(),
                    replay: ReplayClassification::Unavailable,
                };
            }
        };
        self.invocations.borrow_mut().push(amount);
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
        self.releases.set(self.releases.get() + 1);
    }
}

/// Everything one provider path produced, in the shape the probes assert on.
struct ProviderPathObservation {
    /// The source-level value the execution produced, when it produced one.
    value: Option<ReplacementValue>,
    /// The stable diagnostic code the execution refused with, when it refused.
    error_code: Option<&'static str>,
    /// The status of the single RFC 104 operation receipt the run emitted, when it emitted one.
    receipt_status: Option<ReceiptStatus>,
    /// The keys whose values that receipt withheld.
    redacted_keys: Vec<String>,
    /// Whether the receipt's own authority decision allowed the operation.
    authority_allowed: Option<bool>,
    /// The amounts the ledger was actually asked to charge.
    invocations: Vec<i64>,
    /// How many settlement handles the ledger released.
    releases: usize,
    /// The lifecycle transitions the run recorded, in order.
    lifecycle: Vec<&'static str>,
    /// The backend execution receipts the run finalized, as `(outcome, referenced receipt sequence id)`.
    backend_executions: Vec<(&'static str, u64)>,
    /// Every comparison state those backend receipts declared.
    comparison_reasons: Vec<String>,
}

/// Lower the ledger fixture, run `settle("acct-1", 250)` against a fixture host, and report what happened.
///
/// The catalog key is the operation's canonical identity, minted the way lowering mints it. Nothing tells the call
/// site anything: admission travels entirely through that identity.
fn observe_provider_path(
    behavior: LedgerBehavior,
    mode: AuthorityMode,
    grant_capability: bool,
) -> Result<ProviderPathObservation, String> {
    let tokens = lexer::lex(PROVIDER_CASE_SRC).map_err(|errors| format!("provider fixture lex failure: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("provider fixture parse failure: {errors:?}"))?;
    let module_path = vec!["app".to_string()];
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("provider fixture typecheck failure: {errors:?}"))?;

    // Admission is projected from a published provider manifest through a selected `ProviderPlan`, never
    // hand-filled into the lowering catalog -- which #1213 made private precisely so a consumer cannot invent
    // admission a real producer could not have published.
    let descriptors: Vec<ProviderOperationMetadata> = checker
        .type_info()
        .declarations
        .provider_operations
        .values()
        .map(|declared| ProviderOperationMetadata {
            operation: declared.operation.clone(),
            required_capability: declared.required_capability.clone(),
            runtime_requirements: declared.runtime_requirements.clone(),
        })
        .collect();
    let descriptor = descriptors
        .first()
        .ok_or("the provider fixture declares no checked provider operation")?;
    let operation = descriptor.operation.clone();
    let required_capability = descriptor.required_capability.clone();
    let namespace_claims: BTreeSet<Vec<String>> = descriptors
        .iter()
        .filter_map(|descriptor| descriptor.operation.module_path().map(ToOwned::to_owned))
        .collect();

    let mut manifest = LibraryManifest::new("corpus_provider", "0.1.0");
    manifest.contract_metadata.provider = CompiledProviderMetadata {
        operation_descriptors: descriptors,
        ..CompiledProviderMetadata::default()
    };
    let provider_plan = ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "corpus_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "fixture:corpus-provider".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Compiler,
            authority: NamespaceAuthority::Compiler,
            namespace_claims: namespace_claims.clone(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        namespace_claims,
    )
    .map_err(|error| error.to_string())?;
    let module =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)
            .map_err(|error| format!("provider fixture lowering failure: {error}"))?;

    let host = Rc::new(CorpusLedgerHost::new(operation, behavior));
    let authority = StaticAuthority::new(mode, grant_capability.then_some(required_capability));
    let providers = ProviderRuntime::new(Rc::new(authority), host.clone());
    let executed = execute_free_function_with_providers(
        &module,
        "settle",
        &[ReplacementValue::Str("acct-1".to_string()), ReplacementValue::Int(250)],
        &providers,
    );

    let receipts = providers.operation_receipts();
    let receipt = receipts.first();
    // A receipt that contradicts its own fields would make every other assertion here meaningless, so the
    // contract check runs before anything is reported.
    if let Some(receipt) = receipt
        && let Err(violation) = receipt.validate()
    {
        return Err(format!("the emitted operation receipt contradicts itself: {violation}"));
    }
    let executions = providers.provider_executions();
    Ok(ProviderPathObservation {
        value: executed.as_ref().ok().map(|execution| execution.value.clone()),
        error_code: executed.as_ref().err().map(ReplacementExecutionError::diagnostic_code),
        receipt_status: receipt.map(|receipt| receipt.status()),
        redacted_keys: receipt.map(|receipt| receipt.redacted_keys()).unwrap_or_default(),
        authority_allowed: receipt.map(|receipt| receipt.authority().is_allowed()),
        invocations: host.invocations.borrow().clone(),
        releases: host.releases.get(),
        lifecycle: providers
            .lifecycle_evidence()
            .into_iter()
            .map(|event| event.event)
            .collect(),
        backend_executions: executions
            .iter()
            .map(|record| {
                let projection = record.projection();
                (projection.outcome, projection.operation_receipt_sequence_id)
            })
            .collect(),
        comparison_reasons: executions
            .iter()
            .map(|record| record.projection().comparison_reason)
            .collect(),
    })
}

/// Confirm that every backend execution the observation recorded declared an explicitly non-green comparison.
fn provider_comparison_is_explicitly_non_green(observation: &ProviderPathObservation) -> Option<String> {
    if observation.comparison_reasons.is_empty() {
        return Some("a provider path recorded no backend execution receipt at all".to_string());
    }
    observation
        .comparison_reasons
        .iter()
        .find(|reason| reason.as_str() != PROVIDER_COMPARISON_UNAVAILABLE_REASON)
        .map(|reason| format!("a provider execution claimed a comparison state it cannot support: {reason}"))
}

/// Turn a provider path observation plus a claim about it into a corpus outcome, without panicking.
fn provider_outcome(
    observation: Result<ProviderPathObservation, String>,
    claim: impl FnOnce(&ProviderPathObservation) -> Option<String>,
) -> ComparisonOutcome {
    let observation = match observation {
        Ok(observation) => observation,
        Err(reason) => return ComparisonOutcome::Incompatible { reason },
    };
    match provider_comparison_is_explicitly_non_green(&observation).or_else(|| claim(&observation)) {
        Some(detail) => ComparisonOutcome::Mismatch { detail },
        None => ComparisonOutcome::Match,
    }
}

/// An allowed invocation runs the provider and binds a backend receipt to the operation receipt it describes.
pub(super) fn case_provider_allowed_invocation() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Settle, AuthorityMode::Governed, true),
        |observed| {
            if observed.value != Some(ReplacementValue::Int(255)) {
                return Some(format!(
                    "an allowed charge must produce the provider's settled value, got {:?}",
                    observed.value
                ));
            }
            if observed.receipt_status != Some(ReceiptStatus::Allowed) {
                return Some(format!(
                    "expected an allowed receipt, got {:?}",
                    observed.receipt_status
                ));
            }
            if observed.backend_executions != vec![("allowed", 0)] {
                return Some(format!(
                    "the backend receipt must reference the operation receipt it describes, got {:?}",
                    observed.backend_executions
                ));
            }
            None
        },
    )
}

/// A governed denial emits a denied receipt, reports a source-owned diagnostic, and never reaches the provider.
pub(super) fn case_provider_governed_denial() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Settle, AuthorityMode::Governed, false),
        |observed| {
            if !observed.invocations.is_empty() {
                return Some(format!(
                    "a denied operation must never reach the provider, but it was invoked with {:?}",
                    observed.invocations
                ));
            }
            if observed.error_code != Some("INCAN-R1156-DENIED") {
                return Some(format!(
                    "a denial must report its own source-owned diagnostic, got {:?}",
                    observed.error_code
                ));
            }
            if observed.receipt_status != Some(ReceiptStatus::Denied) || observed.authority_allowed != Some(false) {
                return Some(format!(
                    "a denial is a recorded outcome over a refusing decision, got {:?}/{:?}",
                    observed.receipt_status, observed.authority_allowed
                ));
            }
            if observed.lifecycle != vec!["denied"] {
                return Some(format!(
                    "a denial acquires nothing, so it has nothing to release: {:?}",
                    observed.lifecycle
                ));
            }
            None
        },
    )
}

/// A provider failure keeps its allowing authority decision and reports its own diagnostic, not a denial's.
pub(super) fn case_provider_operation_failure() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Decline, AuthorityMode::Governed, true),
        |observed| {
            if observed.error_code != Some("INCAN-R1156-PROVIDER") {
                return Some(format!(
                    "a provider failure is not a denial and reports its own code, got {:?}",
                    observed.error_code
                ));
            }
            if observed.receipt_status != Some(ReceiptStatus::Failed) || observed.authority_allowed != Some(true) {
                return Some(format!(
                    "a failure keeps its allowing authority decision, got {:?}/{:?}",
                    observed.receipt_status, observed.authority_allowed
                ));
            }
            if observed.invocations != vec![250] {
                return Some(format!(
                    "a failure happens after the provider was reached, got {:?}",
                    observed.invocations
                ));
            }
            None
        },
    )
}

/// A withheld attribute classifies the receipt as redacted without changing what the operation returned.
pub(super) fn case_provider_redaction_classification() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::SettleWithSecretAccount, AuthorityMode::Governed, true),
        |observed| {
            if observed.receipt_status != Some(ReceiptStatus::Redacted) {
                return Some(format!(
                    "a receipt with a withheld value must stop claiming it recorded everything, got {:?}",
                    observed.receipt_status
                ));
            }
            if observed.redacted_keys != vec!["ledger.account".to_string()] {
                return Some(format!(
                    "a redacted attribute keeps its key, got {:?}",
                    observed.redacted_keys
                ));
            }
            if observed.value != Some(ReplacementValue::Int(255)) {
                return Some(format!(
                    "redaction changes what is recorded, not what the operation returned, got {:?}",
                    observed.value
                ));
            }
            if observed
                .backend_executions
                .iter()
                .any(|(outcome, _)| *outcome != "redacted")
            {
                return Some(format!(
                    "the backend receipt must record the classification, got {:?}",
                    observed.backend_executions
                ));
            }
            None
        },
    )
}

/// An invocation that failed still releases what it acquired, exactly once and after the failure.
pub(super) fn case_provider_lifecycle_cleanup() -> ComparisonOutcome {
    provider_outcome(
        observe_provider_path(LedgerBehavior::Decline, AuthorityMode::Governed, true),
        |observed| {
            if observed.lifecycle != vec!["invoked", "failed", "released"] {
                return Some(format!(
                    "cleanup follows the outcome it cleans up after and never precedes the invocation: {:?}",
                    observed.lifecycle
                ));
            }
            if observed.releases != 1 {
                return Some(format!(
                    "an invocation that failed still releases what it acquired, exactly once; got {}",
                    observed.releases
                ));
            }
            None
        },
    )
}
