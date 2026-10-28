//! Diagnostic observations of actual checked-body construction, never cache authority.

use incan_semantics_core::CompilerNodeId;

/// Observe one body immediately before its infallible construction. Restored or merely copied dependency bodies
/// do not pass this boundary. The locator is physical and diagnostic, not a reuse identity.
pub(super) fn built_body(identity: &CompilerNodeId, kind: &'static str) {
    tracing::trace!(target: "incan::invocation", phase = "body_ir", action = "built", kind,
        identity = identity.path(), "compiler work");
}

/// Observe a nested closure or generator at its own producer. Nested work is reported separately from top-level
/// Body IR frames because its source identity and later native lifting differ from its enclosing function.
pub(super) fn nested_body(module: &str, input: impl std::fmt::Debug, kind: &'static str) {
    tracing::trace!(target: "incan::invocation", phase = "body_ir", action = "nested", kind, module,
        input = ?input, "compiler work");
}
