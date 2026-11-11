//! Bootstrap entry for DD-0015 compiler-record activations (#2256).
//!
//! The worker protocol reaches this adapter before individual producer families migrate. An unmigrated family is an
//! explicit compiler fault, never a whole-program fallback. Family implementations replace these faults one at a
//! time and must construct their evaluation reader through `internal_models::prepared_reader` before entering the
//! existing semantic operation. This Rust boundary is removable bootstrap debt under DD-0015 decision 4 and BE-32;
//! scheduling, storage, preparation policy, and the record service remain Incan-owned.

use std::env;

use incan_worker_host::{CompilerFault, RecordMessage};
use sha2::{Digest, Sha256};

/// Refuse an activation whose producer family has not migrated to the record-scoped compiler entry yet.
///
/// The request-specific root lets the engine correlate the fault with the immutable activation input. No legacy
/// checker or cache is consulted, and no staged value or witness set is published.
pub(crate) fn activate(request: RecordMessage) -> RecordMessage {
    match request {
        RecordMessage::Evaluate(value) => evaluate_local(value).unwrap_or_else(fault),
        RecordMessage::Prepare(value) => fault(format!("unsupported-producer:{}:{}", value.stage, value.demand_root)),
        _ => fault("protocol-fault".to_owned()),
    }
}

/// Resolve one prepared local activation, enter the frontend's single-owner scope and return content roots.
///
/// Snapshot resolution, `prepared_reader`, `StoreRecordSource` and checker-record decoding remain Incan-owned. This
/// adapter receives only their decoded source/declaration/environment values; it never receives a `TypeChecker` or
/// invokes `check_program_internal`.
fn evaluate_local(value: incan_worker_host::Evaluate) -> Result<RecordMessage, String> {
    let home = env::var("INCAN_HOME").map_err(|_| "local record activation requires INCAN_HOME".to_owned())?;
    let staging_home = home.clone();
    let snapshot = value.snapshot.clone();
    let inputs = incan_worker_host::resolve_local_activation(home, value).map_err(|error| error.to_string())?;
    let rows = incan_frontend::checker_records::produce_local_record(
        &inputs.source,
        &inputs.owner,
        inputs.declaration,
        inputs.environment,
    )
    .map_err(|error| format!("local producer refused: {error:?}"))?;
    let record = internal_models::from_native_local_fields(&rows)
        .map_err(|error| format!("local record codec refused: {error:?}"))?;
    let encoded =
        internal_models::encode_record(record).map_err(|error| format!("local record encoding refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced = incan_worker_host::stage_local_result(staging_home, snapshot, &encoded, inputs.receipt, &semantic)
        .map_err(|error| error.to_string())?;
    Ok(RecordMessage::Produced(produced))
}

/// Convert one adapter refusal into the protocol's typed compiler-fault reply.
fn fault(fault_root: String) -> RecordMessage {
    RecordMessage::CompilerFault(CompilerFault { fault_root })
}

/// Return a lowercase SHA-256 content root for one staged response payload.
fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
