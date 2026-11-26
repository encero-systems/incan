//! Diagnostic records of actual lowering requests, for counting repeated work.

use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::Body;

use crate::symbols::type_component;
use crate::trace_channel::emit;

/// Record an actual lowering request, including repeated concrete instantiations.
///
/// 1. Keep normal builds unobserved.
/// 2. Identify the checked frame and its concrete request.
/// 3. Write only a diagnostic, to the tracing channel's `redo` category; a closed stream cannot change compilation.
pub fn record_lowering(enabled: bool, body: &Body, frame: &str, arguments: &[IncanType], file: &str) {
    // 1. Native timing selects compiler work diagnostics.
    if !enabled {
        return;
    }
    // 2. The generated frame and concrete arguments distinguish physical requests within this invocation.
    let concrete: String = arguments.iter().map(type_component).collect();
    let text = format!(
        "redo-item: lowering\t{}\t{frame}\t{concrete}\t{file}",
        body.direct_call_id.path()
    );
    // 3. A failed diagnostic never changes a compilation verdict or the program's console streams.
    emit("redo", "OVEN_TRACE", &text);
}
