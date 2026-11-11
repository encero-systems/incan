//! Bootstrap entry for DD-0015 compiler-record activations (#2256).
//!
//! The worker protocol reaches this adapter before individual producer families migrate. An unmigrated family is an
//! explicit compiler fault, never a whole-program fallback. Family implementations replace these faults one at a
//! time and must construct their evaluation reader through `internal_models::prepared_reader` before entering the
//! existing semantic operation. This Rust boundary is removable bootstrap debt under DD-0015 decision 4 and BE-32;
//! scheduling, storage, preparation policy, and the record service remain Incan-owned.

use incan_worker_host::{CompilerFault, RecordMessage};

/// Refuse an activation whose producer family has not migrated to the record-scoped compiler entry yet.
///
/// The request-specific root lets the engine correlate the fault with the immutable activation input. No legacy
/// checker or cache is consulted, and no staged value or witness set is published.
pub(crate) fn activate(request: RecordMessage) -> RecordMessage {
    let fault_root = match request {
        RecordMessage::Prepare(value) => format!("unsupported-producer:{}:{}", value.stage, value.demand_root),
        RecordMessage::Evaluate(value) => format!("unsupported-producer:{}", value.work_root),
        _ => "protocol-fault".to_owned(),
    };
    RecordMessage::CompilerFault(CompilerFault { fault_root })
}
