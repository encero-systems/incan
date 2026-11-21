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
