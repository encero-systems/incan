//! Mechanical exchange with the compiler-bound Incan lock policy; no lock decision is implemented here.

use crate::error::{CliError, CliResult};
use serde::Deserialize;

const SCHEMA: &str = "incan.oven.lock-preparation/1";

/// Closed operations returned by the Incan policy; each caller accepts only its own phase's operations.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    /// The request contains a path outside the host's absolute-path contract.
    RelativePaths,
    /// Canonical collection owns unsupported retained layouts and workspace preparation.
    CanonicalPrepared,
    /// A standalone request has no declared or explicit source entrypoint.
    MissingEntry,
    /// Reconcile the original owner session before asking whether publication is reusable.
    Observe,
    /// Canonical collection owns a request whose original session cannot be retained.
    CanonicalCollect,
    /// Return the original successful publication without collecting sources again.
    Reuse,
    /// Publish through the canonical collector with the original session.
    Publish,
    /// Retain this successful publication in the original session's slot.
    Retain,
    /// Publication completed, but its evidence must not be retained.
    Complete,
}

/// Exact response to one request; unknown fields and actions are refused during decoding.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    schema: String,
    request_digest: String,
    status: String,
    action: Option<Action>,
    reason: Option<String>,
}

/// Execute the existing engine exchange, preserving its original compiler binding across evaluation.
///
/// This adapter supplies observations only. Incan owns the decision, and the response cannot be replayed for
/// other facts. Engine replacement during execution refuses rather than retaining an unauthenticated result.
pub(super) fn decide(phase: &str, facts: &serde_json::Value) -> CliResult<Action> {
    let _trace =
        tracing::trace_span!(target: "incan::invocation", "incan_driver::evaluate_lock_policy", phase).entered();
    let (engine, digest) = super::super::native_runtime_inputs::bound_policy_engine()?;
    let mut inputs = super::super::retained_inputs::ModelInputs::default();
    inputs.observe(&engine);
    super::super::native_runtime_inputs::bound_policy_engine()?;
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema": SCHEMA, "phase": phase, "facts": facts,
    }))
    .map_err(|error| CliError::failure(error.to_string()))?;
    let response: Response = super::super::native_runtime_inputs::execute_exchange(&engine, &bytes)?;
    if !inputs.supports_retention()
        || !inputs.current()
        || super::super::file_freshness::digest_file(&engine).map_err(|error| CliError::failure(error.to_string()))?
            != digest
        || response.schema != SCHEMA
        || response.request_digest != oven_store::digest_bytes(&bytes)
    {
        return Err(CliError::failure(
            "lock policy response lost its original engine or request binding",
        ));
    }
    match (response.status.as_str(), response.action, response.reason) {
        ("selected", Some(action), None) => Ok(action),
        ("refused", None, Some(reason)) => Err(CliError::failure(format!("lock policy refused: {reason}"))),
        _ => Err(CliError::failure("lock policy response is incomplete or contradictory")),
    }
}
