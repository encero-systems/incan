//! Diagnostic observations of actual checked-body construction, never cache authority.

use incan_semantics_core::CompilerNodeId;

/// Observe one retained top-level body after its producer has completed. Restored dependency bodies do not pass this
/// boundary. The locator is physical and diagnostic, not a reuse identity.
pub(super) fn built_body(identity: &CompilerNodeId) {
    tracing::trace!(target: "incan::invocation", phase = "body_ir", action = "built", kind = "body",
        identity = identity.path(), "compiler work");
}
