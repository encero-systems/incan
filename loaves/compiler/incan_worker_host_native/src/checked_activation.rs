//! Resolve shared checking-family activation envelopes and stage their canonical outputs.

use internal_models_native::activation_envelopes::{
    PREPARED_FAMILY, WORK_FAMILY, decode_prepared_inputs, decode_ready_work,
};
use internal_models_native::read_receipts::encode_read_receipt;
use internal_models_native::record_scope::ReadReceipt;
use internal_models_native::record_service::{PreparedInputs, ReadyWork};

use crate::record_protocol::{Evaluate, Produced};
use crate::record_snapshot::{ResolvedSnapshot, resolve_snapshot};

/// Decoded engine envelopes and the selected store view shared by checking-family readers.
pub struct ResolvedCheckingActivation {
    pub snapshot: ResolvedSnapshot,
    pub ready: ReadyWork,
    pub prepared: PreparedInputs,
}

/// Keep the owner on its own line when source projection trims the module header.
pub fn projected_owner_source(header: &str, owner: &str) -> String {
    if header.is_empty() || header.ends_with('\n') {
        return format!("{header}{owner}");
    }
    format!("{header}\n{owner}")
}

/// Resolve and decode one Evaluate message's work and preparation roots exactly once.
pub fn resolve_checking_activation(home: &str, evaluation: &Evaluate) -> Result<ResolvedCheckingActivation, String> {
    if evaluation.reservation.is_empty() || evaluation.cancellation_generation < 0 {
        return Err("checking evaluation requires a reservation and cancellation generation".to_owned());
    }
    let resolved = resolve_snapshot(home, &evaluation.snapshot).map_err(|error| error.message())?;
    let work = resolved
        .checking_envelope(WORK_FAMILY, &evaluation.work_root)
        .map_err(|error| error.message())?;
    let ready = decode_ready_work(&work)?;
    let preparation = resolved
        .checking_envelope(PREPARED_FAMILY, &evaluation.prepared_root)
        .map_err(|error| error.message())?;
    let prepared = decode_prepared_inputs(&preparation)?;
    if prepared.snapshot.root.digest != evaluation.snapshot {
        return Err("checking preparation belongs to another pinned input snapshot".to_owned());
    }
    Ok(ResolvedCheckingActivation {
        snapshot: resolved,
        ready,
        prepared,
    })
}

/// Stage one checking-family value and its canonical evidence pages, then return only their content roots.
pub fn stage_checking_result(
    home: &str,
    snapshot: &str,
    value: &[u8],
    receipt: &ReadReceipt,
    semantic: &str,
) -> Result<Produced, String> {
    let receipt_bytes = encode_read_receipt(receipt)?;
    let counter = format!("evaluations=1\nsemantic={semantic}").into_bytes();
    let resolved = resolve_snapshot(home, snapshot).map_err(|error| error.message())?;
    let roots = resolved
        .stage_compiler_outputs(value, &receipt_bytes, b"", &counter)
        .map_err(|error| error.message())?;
    Ok(Produced {
        staged_root: roots.value,
        witness_root: roots.receipt,
        diagnostic_root: roots.diagnostic,
        counter_root: roots.counter,
    })
}
