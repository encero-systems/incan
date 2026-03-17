//! Desugaring interfaces for vocab companion crates.
//!
//! The metadata contract and the desugaring API intentionally live in the same crate so companion crates can depend on
//! one stable boundary for both vocabulary registration and syntax-lowering hooks.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use crate::ast::{IncanExpr, IncanStatement, Span, VocabSyntaxNode};

/// Desugarer runtime artifact format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DesugarerArtifactKind {
    /// WASM module artifact loaded at consumer compile time.
    #[default]
    WasmModule,
}

/// Serializable metadata describing how a companion crate emits its desugarer artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DesugarerMetadata {
    /// Artifact kind to produce and package.
    pub artifact_kind: DesugarerArtifactKind,
    /// WASM ABI contract version for compiler/desugarer runtime interop.
    #[cfg_attr(feature = "serde", serde(default = "default_wasm_desugar_abi_version"))]
    pub abi_version: u32,
    /// Target triple used to build the artifact.
    pub target: String,
    /// Cargo profile used to build the artifact (`release` by default).
    pub profile: String,
    /// Optional explicit output file name.
    pub file_name: Option<String>,
    /// Runtime entrypoint symbol used by the compiler.
    pub entrypoint: String,
}

impl Default for DesugarerMetadata {
    fn default() -> Self {
        Self {
            artifact_kind: DesugarerArtifactKind::WasmModule,
            abi_version: crate::WASM_DESUGAR_ABI_VERSION,
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            file_name: None,
            entrypoint: "desugar_block".to_string(),
        }
    }
}

#[cfg(feature = "serde")]
fn default_wasm_desugar_abi_version() -> u32 {
    crate::WASM_DESUGAR_ABI_VERSION
}

impl DesugarerMetadata {
    /// Create the default Wasm packaging metadata used by library desugarers.
    #[must_use]
    pub fn wasm() -> Self {
        Self::default()
    }

    /// Override the target triple used to build the runtime artifact.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }

    /// Override the Cargo profile used to build the runtime artifact.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    /// Override the packaged output file name.
    #[must_use]
    pub fn with_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    /// Override the runtime entrypoint symbol.
    #[must_use]
    pub fn with_entrypoint(mut self, entrypoint: impl Into<String>) -> Self {
        self.entrypoint = entrypoint.into();
        self
    }

    /// Override the desugarer ABI version.
    #[must_use]
    pub fn with_abi_version(mut self, abi_version: u32) -> Self {
        self.abi_version = abi_version;
        self
    }
}

/// Context supplied to a desugar invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DesugarRequest {
    /// DSL syntax node to desugar.
    pub node: VocabSyntaxNode,
    /// Optional module path for diagnostics.
    pub module_path: Option<String>,
}

/// Output produced by a DSL desugarer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DesugarOutput {
    /// Lowered statements to splice into compiler AST.
    Statements(Vec<IncanStatement>),
    /// Lowered expression to splice into an expression position.
    Expression(IncanExpr),
}

impl Default for DesugarOutput {
    fn default() -> Self {
        Self::Statements(Vec::new())
    }
}

/// Result payload returned by a context-aware desugar call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DesugarResponse {
    /// Lowered output to splice back into compiler AST.
    pub output: DesugarOutput,
}

impl DesugarResponse {
    /// Wrap a statement list as a desugar response.
    #[must_use]
    pub fn statements(statements: Vec<IncanStatement>) -> Self {
        Self {
            output: DesugarOutput::Statements(statements),
        }
    }

    /// Wrap one expression as a desugar response.
    #[must_use]
    pub fn expression(expression: IncanExpr) -> Self {
        Self {
            output: DesugarOutput::Expression(expression),
        }
    }
}

/// Error returned by a library-provided desugarer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesugarError {
    /// Human-readable explanation of what went wrong.
    pub message: String,
    /// Optional source span tied to the desugaring failure.
    pub span: Option<Span>,
}

impl DesugarError {
    /// Create a desugaring error without span information.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }

    /// Create a desugaring error associated with a source span.
    #[must_use]
    pub fn with_span(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
        }
    }
}

impl Display for DesugarError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(span) = self.span {
            write!(f, "{} at {}..{}", self.message, span.start, span.end)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl Error for DesugarError {}

/// Trait implemented by library-provided DSL desugarers.
///
/// A desugarer receives the stable public AST surface from this crate and returns ordinary Incan syntax that the
/// compiler can continue processing.
pub trait VocabDesugarer: Send + Sync {
    /// Desugar one library-defined syntax node into ordinary Incan syntax.
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError>;

    /// Desugar one library-defined syntax node with request context.
    ///
    /// Implementors may override this for richer runtime behavior.
    fn desugar_with_context(&self, request: &DesugarRequest) -> Result<DesugarResponse, DesugarError> {
        let output = self.desugar(&request.node)?;
        Ok(DesugarResponse { output })
    }
}

/// Execute one serialized desugar request with a default-constructed desugarer.
///
/// This helper powers the standard WASM export macro and keeps request/response decoding logic in one canonical place.
#[cfg(feature = "serde")]
pub fn execute_desugar_request<D>(request_json: &[u8]) -> Result<Vec<u8>, String>
where
    D: VocabDesugarer + Default,
{
    let request = serde_json::from_slice::<DesugarRequest>(request_json)
        .map_err(|err| format!("failed to parse desugar request json: {err}"))?;
    let desugarer = D::default();
    let response = desugarer
        .desugar_with_context(&request)
        .map_err(|err| format!("desugarer failed: {err}"))?;
    serde_json::to_vec(&response).map_err(|err| format!("failed to serialize desugar response json: {err}"))
}

/// High-level registration for a library-provided desugarer.
///
/// The common path is `DesugarerRegistration::new(MyDesugarer)` and then, only if needed, chaining metadata overrides
/// such as `.with_target(...)` or `.with_entrypoint(...)`.
pub struct DesugarerRegistration {
    metadata: DesugarerMetadata,
    desugarer: Arc<dyn VocabDesugarer>,
}

impl DesugarerRegistration {
    /// Register one Rust desugarer using the default Wasm packaging metadata.
    #[must_use]
    pub fn new<D>(desugarer: D) -> Self
    where
        D: VocabDesugarer + 'static,
    {
        Self {
            metadata: DesugarerMetadata::default(),
            desugarer: Arc::new(desugarer),
        }
    }

    /// Replace the packaging metadata for this registration.
    #[must_use]
    pub fn with_metadata(mut self, metadata: DesugarerMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Override the target triple used for packaging.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.metadata = self.metadata.with_target(target);
        self
    }

    /// Override the Cargo profile used for packaging.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.metadata = self.metadata.with_profile(profile);
        self
    }

    /// Override the packaged output file name.
    #[must_use]
    pub fn with_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.metadata = self.metadata.with_file_name(file_name);
        self
    }

    /// Override the runtime entrypoint symbol.
    #[must_use]
    pub fn with_entrypoint(mut self, entrypoint: impl Into<String>) -> Self {
        self.metadata = self.metadata.with_entrypoint(entrypoint);
        self
    }

    /// Override the desugarer ABI version.
    #[must_use]
    pub fn with_abi_version(mut self, abi_version: u32) -> Self {
        self.metadata = self.metadata.with_abi_version(abi_version);
        self
    }

    /// Return the serialized artifact metadata derived from this registration.
    #[must_use]
    pub fn metadata(&self) -> &DesugarerMetadata {
        &self.metadata
    }

    /// Return the registered Rust desugarer implementation.
    #[must_use]
    pub fn desugarer(&self) -> &dyn VocabDesugarer {
        self.desugarer.as_ref()
    }
}
