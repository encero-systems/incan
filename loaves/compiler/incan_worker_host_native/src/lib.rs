//! The compiler worker host in Rust, side by side with the Incan `incan_worker_host` until its swap.
//!
//! The host listens on the worker's private socket, admits one request at a time and applies the worker's lifecycle;
//! the platform's sockets and trace channel are reached through `incan_worker_host_bindings`. The wire protocols are
//! the `incan.compiler-worker/1` compile documents, the `incan.compiler-worker/2` record activations, and the
//! version-three typed Crumb packets a `cw3-` worker generation speaks. Every frame is byte-identical to the reference
//! worker host's, refusal texts included.

#![forbid(unsafe_code)]

pub mod activation_inputs;
pub mod checked_activation;
pub mod checking_preparation;
pub mod endpoint;
pub mod host;
mod json_text;
pub mod lifecycle;
pub mod namespace_frames;
pub mod protocol;
pub mod record_directory;
pub mod record_protocol;
pub mod record_snapshot;
pub mod worker_packets;

pub use activation_inputs::{
    FinalActivationInputs, LocalActivationInputs, read_final_activation, read_local_activation,
    resolve_local_activation, stage_local_result,
};
pub use checked_activation::{ResolvedCheckingActivation, resolve_checking_activation, stage_checking_result};
pub use checking_preparation::{completed_checker_key, prepare_checking};
pub use endpoint::{CallerWatch, abandon};
pub use host::{
    WorkerActivation, WorkerHost, caller_watch, complete, complete_record, hands_rustc_off, next_activation,
    next_request, selected_home, start,
};
pub use lifecycle::ActivationOutcome;
pub use namespace_frames::{NamespaceFrameReader, namespace_frame_reader, read_namespace_frame};
pub use protocol::{CompileRequest, SCHEMA, WorkerCommand};
pub use record_directory::{DirectoryAnswer, DirectoryMember, directory_member_key, read_directory};
pub use record_protocol::{
    CompilerFault, Evaluate, NeedPrerequisite, Prepare, Prepared, Produced, RECORD_SCHEMA, RecordAdmission,
    RecordMessage, RecordMessageKind, SemanticRefusal, accept_record_reply, admit_record, decode_record_message,
    encode_record_message,
};
pub use record_snapshot::{
    ResolvedSnapshot, SNAPSHOT_FORMAT_PROFILE, SnapshotFault, SnapshotFaultKind, SnapshotHandle, StagedCompilerOutputs,
    StoreView, canonical_roots, encode_snapshot_handle, encode_snapshot_record, encode_store_view,
    read_snapshot_record, resolve_snapshot, snapshot_handle_digest, snapshot_record_key,
};

#[cfg(test)]
mod activation_tests;
#[cfg(test)]
mod escape_golden;
#[cfg(test)]
mod escape_tests;
#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod number_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod surface_tests;
#[cfg(test)]
mod wire_golden;
#[cfg(test)]
mod wire_tests;
