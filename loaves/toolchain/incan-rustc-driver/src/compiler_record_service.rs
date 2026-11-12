//! Bootstrap entry for DD-0015 compiler-record activations (#2256).
//!
//! The worker protocol reaches this adapter before individual producer families migrate. An unmigrated family is an
//! explicit compiler fault, never a whole-program fallback. Family implementations replace these faults one at a
//! time and must construct their evaluation reader through `internal_models::prepared_reader` before entering the
//! existing semantic operation. This Rust boundary is removable bootstrap debt under DD-0015 decision 4 and BE-32;
//! scheduling, storage, preparation policy, and the record service remain Incan-owned.

use incan_worker_host::{CompilerFault, RecordMessage};
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
        RecordMessage::Prepare(value) => fault(format!("unsupported-producer:{}:{}", value.stage, value.demand_root)),
        _ => fault("protocol-fault".to_owned()),
    }
}

/// Resolve shared activation envelopes once, then select the one compiler-owned producer for the closed checking
/// family. Unsupported families fault without entering a checker operation.
fn evaluate(value: incan_worker_host::Evaluate, home: &str) -> Result<RecordMessage, String> {
    let snapshot = value.snapshot.clone();
    let activation =
        incan_worker_host::resolve_checking_activation(home.to_owned(), &value).map_err(|error| error.to_string())?;
    match activation.ready.output.family.as_str() {
        "local" => evaluate_local(activation, snapshot, home),
        "final" => evaluate_final(activation, snapshot, home),
        "body-ir-seed" => evaluate_body_ir_seed(activation, snapshot, home),
        family => Err(format!("unsupported-producer:{family}:{}", value.work_root)),
    }
}

/// Restore one settled owner and enter the existing compiler-owned free-function lowering operation.
fn evaluate_body_ir_seed(
    activation: incan_worker_host::ResolvedCheckingActivation,
    snapshot: String,
    home: &str,
) -> Result<RecordMessage, String> {
    let inputs = incan_worker_host::read_body_ir_seed_activation(activation).map_err(|error| error.to_string())?;
    let declarations = environment_values(
        inputs.declarations,
        inputs.declaration_names,
        inputs.declaration_identities,
    )?;
    let module_path = source_module_path(&inputs.source_path)?;
    let identity = inputs
        .r#final
        .first()
        .and_then(|header| header.get(1))
        .cloned()
        .ok_or("Body IR seed final identity is absent")?;
    let encoded = incan_frontend::checker_records::produce_body_ir_seed(
        &inputs.source,
        &inputs.source_path,
        &module_path,
        &inputs.owner,
        &identity,
        inputs.r#final,
        declarations,
    )
    .map_err(|error| format!("Body IR seed producer refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced =
        incan_worker_host::stage_checking_result(home.to_owned(), snapshot, &encoded, inputs.receipt, &semantic)
            .map_err(|error| error.to_string())?;
    Ok(RecordMessage::Produced(produced))
}

/// Derive the compiler's logical module segments for the initial graph-eligible `src` source layout.
fn source_module_path(source_path: &str) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(source_path);
    let mut components = path
        .components()
        .map(|component| component.as_os_str().to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("Body IR seed source path is not UTF-8")?;
    if components.first().map(String::as_str) != Some("src") || components.len() < 2 {
        return Err("Body IR seed source path is outside the initial src layout".to_owned());
    }
    components.remove(0);
    let last = components.last_mut().ok_or("Body IR seed source module is absent")?;
    *last = std::path::Path::new(last)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or("Body IR seed source module is invalid")?;
    Ok(incan_frontend::module::canonicalize_source_module_segments(&components))
}

/// Resolve one prepared local activation, enter the frontend's single-owner scope and return content roots.
///
/// Snapshot resolution, `prepared_reader`, `StoreRecordSource` and checker-record decoding remain Incan-owned. This
/// adapter receives only their decoded source/declaration/environment values; it never receives a `TypeChecker` or
/// invokes `check_program_internal`.
fn evaluate_local(
    activation: incan_worker_host::ResolvedCheckingActivation,
    snapshot: String,
    home: &str,
) -> Result<RecordMessage, String> {
    let inputs = incan_worker_host::read_local_activation(activation).map_err(|error| error.to_string())?;
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
        internal_models::encode_record(record).map_err(|error| format!("local record encoding refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced = incan_worker_host::stage_local_result(home.to_owned(), snapshot, &encoded, inputs.receipt, semantic)
        .map_err(|error| error.to_string())?;
    Ok(RecordMessage::Produced(produced))
}

/// Decode one local predecessor, enter its single-owner settlement operation and stage the canonical final value.
fn evaluate_final(
    activation: incan_worker_host::ResolvedCheckingActivation,
    snapshot: String,
    home: &str,
) -> Result<RecordMessage, String> {
    let inputs = incan_worker_host::read_final_activation(activation).map_err(|error| error.to_string())?;
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
        internal_models::encode_record(record).map_err(|error| format!("final record encoding refused: {error:?}"))?;
    let semantic = format!("sha256:{}", digest(&encoded));
    let produced =
        incan_worker_host::stage_checking_result(home.to_owned(), snapshot, &encoded, inputs.receipt, &semantic)
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

#[cfg(test)]
mod tests {
    use super::source_module_path;

    #[test]
    fn body_ir_seed_source_paths_preserve_compiler_module_identity() -> Result<(), String> {
        if source_module_path("src/main.incn")? != ["main"] {
            return Err("main seed module identity changed".to_owned());
        }
        if source_module_path("src/models/mod.incn")? != ["models"] {
            return Err("directory seed module identity changed".to_owned());
        }
        if source_module_path("generated/main.incn").is_ok() {
            return Err("seed activation accepted a source outside the initial src layout".to_owned());
        }
        Ok(())
    }
}
