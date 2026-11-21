//! The crate root exports every name the Incan worker host's root exports (`incan_worker_host/src/lib.incn`), so the
//! driver and Oven keep their import paths when this crate takes the worker host's place.

#[allow(unused_imports)]
use crate::{
    ActivationOutcome, CallerWatch, CompileRequest, CompilerFault, DirectoryAnswer, DirectoryMember, Evaluate,
    FinalActivationInputs, LocalActivationInputs, NamespaceFrameReader, NeedPrerequisite, Prepare, Prepared, Produced,
    RECORD_SCHEMA, RecordAdmission, RecordMessage, RecordMessageKind, ResolvedCheckingActivation, ResolvedSnapshot,
    SCHEMA, SNAPSHOT_FORMAT_PROFILE, SemanticRefusal, SnapshotFault, SnapshotFaultKind, SnapshotHandle,
    StagedCompilerOutputs, StoreView, WorkerActivation, WorkerCommand, WorkerHost, abandon, accept_record_reply,
    admit_record, caller_watch, canonical_roots, complete, complete_record, completed_checker_key,
    decode_record_message, directory_member_key, encode_record_message, encode_snapshot_handle, encode_snapshot_record,
    encode_store_view, hands_rustc_off, namespace_frame_reader, next_activation, next_request, prepare_checking,
    read_directory, read_final_activation, read_local_activation, read_namespace_frame, read_snapshot_record,
    resolve_checking_activation, resolve_local_activation, resolve_snapshot, selected_home, snapshot_handle_digest,
    snapshot_record_key, stage_checking_result, stage_local_result, start,
};

#[test]
fn the_crate_root_keeps_the_reference_surface() {
    assert_eq!(SCHEMA, "incan.compiler-worker/1");
    assert_eq!(RECORD_SCHEMA, "incan.compiler-worker/2");
    assert_eq!(SNAPSHOT_FORMAT_PROFILE, "incan.compiler-records/1");
}

/// The names the Incan `incan_worker_host` binding re-exports for Oven, reachable from the crate root, where an
/// Incan caller can construct and call them.
#[allow(unused_imports)]
use crate::{
    Admission, Fault, MISMATCH, PANIC_STATUS, PREFIX, Reply, STORE_VIEW_SCHEMA, SelectedStoreView, WorkerCompletion,
    admit, compile_frame, encode_store_binding, portable_store_view, read_completion, read_ready, ready_frame,
    record_frame, reply_frame, select_store_view, staged_key, valid_digest,
};

#[test]
fn a_binding_selects_a_store_view_by_its_path_text() {
    let missing = select_store_view("/nonexistent-runtime-home/bakery.view");
    assert_eq!(missing.err().as_deref(), Some("No such file or directory (os error 2)"));
    assert_eq!(PREFIX, "cw3-");
    assert_eq!(STORE_VIEW_SCHEMA, "incan.store-view/1");
}
