//! Typed failures of provider admission, syntax translation and the native worker protocol.

use crate::ast::Span;
use crate::vocab_ast_bridge::VocabAstBridgeError;

/// Desugaring failures become node-local diagnostics; none is a compiler panic.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VocabDesugarPassError {
    /// A compiler-owned surface could not be translated through its stable syntax boundary.
    #[error("bridge error for vocab block `{keyword}`: {source}")]
    Bridge {
        /// The source keyword whose payload failed translation.
        keyword: String,
        /// The unsupported boundary shape.
        source: VocabAstBridgeError,
    },
    /// A registered dependency does not publish an executable expansion artifact.
    #[error("desugarer resolution failed for keyword `{keyword}`: {message}")]
    Resolution {
        /// The parsed vocabulary keyword.
        keyword: String,
        /// The missing or invalid dependency fact.
        message: String,
    },
    /// The provider's output cannot occupy this source position.
    #[error("desugarer returned an unsupported output position for block keyword `{keyword}`")]
    UnsupportedOutput {
        /// The parsed vocabulary keyword.
        keyword: String,
    },
    /// A native worker was denied, failed, exceeded its deadline or returned invalid public syntax.
    #[error("vocabulary provider `{provider}`: {message}")]
    Native {
        /// The providing Loaf, never the worker's local executable path.
        provider: String,
        /// The failure class and actionable explanation.
        message: String,
        /// A validated fine-grained source location reported by the provider.
        span: Option<Span>,
    },
}
