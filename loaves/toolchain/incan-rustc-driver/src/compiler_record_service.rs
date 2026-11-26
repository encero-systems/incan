//! Bootstrap entry for DD-0015 compiler-record activations (#2256).
//!
//! The worker protocol reaches this adapter before individual producer families migrate. An unmigrated family is an
//! explicit compiler fault, never a whole-program fallback. Family implementations replace these faults one at a
//! time and must construct their evaluation reader through `internal_models::prepared_reader` before entering the
//! existing semantic operation. This Rust boundary is removable bootstrap debt under DD-0015 decision 4 and BE-32;
//! scheduling, storage, preparation policy, and the record service belong to Oven and the worker host.

use incan_worker_host_native::{CompilerFault, RecordMessage};
use sha2::{Digest, Sha256};

/// Refuse an activation whose producer family has not migrated to the record-scoped compiler entry yet.
///
/// The request-specific root lets the engine correlate the fault with the immutable activation input. No legacy
/// checker or cache is consulted, and no staged value or witness set is published.
pub(crate) fn activate(request: RecordMessage, home: &str) -> RecordMessage {
    match request {
        RecordMessage::Evaluate(value) => match evaluate(value, home) {
            Ok(produced) => produced,
            Err(error) => fault(error),
        },
        RecordMessage::Prepare(value)
            if matches!(value.stage.as_str(), "signature" | "check-body" | "local" | "final") =>
        {
            match incan_worker_host_native::prepare_checking(home, &value) {
                Ok(prepared) => RecordMessage::Prepared(prepared),
                Err(error) => fault(error.to_string()),
            }
        }
        RecordMessage::Prepare(value) => fault(format!("unsupported-producer:{}:{}", value.stage, value.demand_root)),
        _ => fault("protocol-fault".to_owned()),
    }
}

/// Resolve shared activation envelopes once, then select the one compiler-owned producer for the closed checking
/// family. Unsupported families fault without entering a checker operation.
fn evaluate(value: incan_worker_host_native::Evaluate, home: &str) -> Result<RecordMessage, String> {
    let snapshot = value.snapshot.clone();
    let activation =
        incan_worker_host_native::resolve_checking_activation(home, &value).map_err(|error| error.to_string())?;
    let output = &activation.ready.output;
    let produced = match output.family.as_str() {
        "declaration" => evaluate_closed_declaration(&activation, snapshot, home),
        "local" => evaluate_local(&activation, snapshot, home),
        "final" => evaluate_final(&activation, snapshot, home),
        family => return Err(format!("unsupported-producer:{family}:{}", value.work_root)),
    }?;
    if let RecordMessage::Produced(done) = &produced {
        tracing::debug!(target: "incan::invocation", owner = %output.identity, family = %output.family,
            root = %done.staged_root, "completed ranked checking producer");
    }
    Ok(produced)
}

/// Execute the earlier namespace signature stage without admitting a body check or an undiscovered supplier.
fn evaluate_closed_declaration(
    activation: &incan_worker_host_native::ResolvedCheckingActivation,
    snapshot: String,
    home: &str,
) -> Result<RecordMessage, String> {
    let inputs = incan_worker_host_native::read_local_activation(activation).map_err(|error| error.to_string())?;
    let Some(rows) =
        incan_frontend::checker_records::produce_closed_declaration(&inputs.source, &inputs.owner, inputs.declaration)
            .map_err(|error| format!("closed declaration producer refused: {error:?}"))?
    else {
        return Err("unsupported-closed-checking-inputs".into());
    };
    let record =
        internal_models::from_native_fields(&rows).map_err(|error| format!("declaration record refused: {error:?}"))?;
    let encoded =
        internal_models::encode_record(&record).map_err(|error| format!("declaration encoding refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced =
        incan_worker_host_native::stage_checking_result(home, &snapshot, &encoded, &inputs.receipt, &semantic)
            .map_err(|error| error.to_string())?;
    Ok(RecordMessage::Produced(produced))
}

/// Resolve one prepared local activation, enter the frontend's single-owner scope and return content roots.
///
/// Snapshot resolution, `prepared_reader`, `StoreRecordSource` and checker-record decoding belong to the worker host.
/// This adapter receives only their decoded source/declaration/environment values; it never receives a `TypeChecker`
/// or invokes `check_program_internal`.
fn evaluate_local(
    activation: &incan_worker_host_native::ResolvedCheckingActivation,
    snapshot: String,
    home: &str,
) -> Result<RecordMessage, String> {
    let inputs = incan_worker_host_native::read_local_activation(activation).map_err(|error| error.to_string())?;
    let environment = environment_values(
        inputs.environment,
        inputs.environment_names,
        inputs.environment_identities,
    )?;
    let rows = incan_frontend::checker_records::produce_local_record(
        &inputs.source,
        &inputs.owner,
        inputs.declaration,
        environment,
    )
    .map_err(|error| format!("local producer refused: {error:?}"))?;
    let record = internal_models::from_native_local_fields(&rows)
        .map_err(|error| format!("local record codec refused: {error:?}"))?;
    let encoded =
        internal_models::encode_record(&record).map_err(|error| format!("local record encoding refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced = incan_worker_host_native::stage_local_result(home, &snapshot, &encoded, &inputs.receipt, &semantic)
        .map_err(|error| error.to_string())?;
    Ok(RecordMessage::Produced(produced))
}

/// Decode one local predecessor, enter its single-owner settlement operation and stage the canonical final value.
fn evaluate_final(
    activation: &incan_worker_host_native::ResolvedCheckingActivation,
    snapshot: String,
    home: &str,
) -> Result<RecordMessage, String> {
    let inputs = incan_worker_host_native::read_final_activation(activation).map_err(|error| error.to_string())?;
    let environment = environment_values(
        inputs.environment,
        inputs.environment_names,
        inputs.environment_identities,
    )?;
    let rows =
        incan_frontend::checker_records::produce_final_record(&inputs.source, &inputs.owner, inputs.local, environment)
            .map_err(|error| format!("final producer refused: {error:?}"))?;
    let record = internal_models::from_native_final_fields(&rows)
        .map_err(|error| format!("final record codec refused: {error:?}"))?;
    let encoded =
        internal_models::encode_record(&record).map_err(|error| format!("final record encoding refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced =
        incan_worker_host_native::stage_checking_result(home, &snapshot, &encoded, &inputs.receipt, &semantic)
            .map_err(|error| error.to_string())?;
    Ok(RecordMessage::Produced(produced))
}

/// Pair decoded declaration rows with the authenticated name and stable identity from their record keys.
fn environment_values(
    values: Vec<Vec<Vec<String>>>,
    names: Vec<String>,
    identities: Vec<String>,
) -> Result<Vec<(String, String, Vec<Vec<String>>)>, String> {
    if values.len() != names.len() || values.len() != identities.len() {
        return Err("producer environment metadata is incomplete".to_owned());
    }
    Ok(values
        .into_iter()
        .zip(names)
        .zip(identities)
        .map(|((fields, name), identity)| (name, identity, fields))
        .collect())
}

/// Convert one adapter refusal into the protocol's typed compiler-fault reply.
fn fault(fault_root: String) -> RecordMessage {
    RecordMessage::CompilerFault(CompilerFault { fault_root })
}

/// Return a lowercase SHA-256 content root for one staged response payload.
fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
