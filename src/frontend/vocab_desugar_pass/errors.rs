use std::path::PathBuf;

use crate::frontend::vocab_ast_bridge::VocabAstBridgeError;

/// Failures produced by the vocab desugaring pass and WASM runtime bridge.
///
/// These are converted into standard compiler diagnostics so callers (CLI/LSP) report actionable errors instead of
/// panicking or leaking runtime internals.
#[derive(Debug, thiserror::Error)]
pub enum VocabDesugarPassError {
    /// Internal AST <-> public vocab AST bridge mapping failed.
    #[error("bridge error for vocab block `{keyword}`: {source}")]
    Bridge {
        /// Parsed keyword that introduced the failing block.
        keyword: String,
        /// Precise mapping error from bridge conversion.
        #[source]
        source: VocabAstBridgeError,
    },
    /// Desugarer artifact could not be resolved from library metadata.
    #[error("desugarer resolution failed for keyword `{keyword}`: {message}")]
    Resolution {
        /// Parsed keyword that needed a desugarer.
        keyword: String,
        /// Human-readable resolution detail.
        message: String,
    },
    /// Desugared output referenced a helper that the provider manifest did not bind.
    #[error("helper binding resolution failed for keyword `{keyword}`: {message}")]
    HelperBinding {
        /// Parsed keyword whose desugared output requested the helper.
        keyword: String,
        /// Human-readable helper-resolution detail.
        message: String,
    },
    /// Artifact file could not be read from disk.
    #[error("failed to read desugarer artifact `{path}`: {source}")]
    ArtifactRead {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Underlying I/O failure.
        source: std::io::Error,
    },
    /// Artifact bytes did not match manifest-provided hash.
    #[error("desugarer artifact checksum mismatch for `{path}`")]
    ChecksumMismatch {
        /// Absolute path to the artifact file.
        path: PathBuf,
    },
    /// WASM module failed to compile.
    #[error("failed to compile wasm module `{path}`: {source}")]
    WasmCompile {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Underlying Wasmtime compile error.
        source: wasmtime::Error,
    },
    /// WASM module failed to instantiate.
    #[error("failed to instantiate wasm module `{path}`: {source}")]
    WasmInstantiate {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Underlying Wasmtime instantiate error.
        source: wasmtime::Error,
    },
    /// Required linear memory export was missing.
    #[error("missing exported memory `memory` in `{path}`")]
    MissingMemory {
        /// Absolute path to the artifact file.
        path: PathBuf,
    },
    /// Configured entrypoint export was not found.
    #[error("missing exported entrypoint `{entrypoint}` in `{path}`")]
    MissingEntrypoint {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Expected export symbol name.
        entrypoint: String,
    },
    /// Exported runtime function shape did not match the required contract.
    #[error("invalid runtime function signature for `{entrypoint}` in `{path}`")]
    InvalidEntrypointSignature {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Entrypoint export symbol name.
        entrypoint: String,
    },
    /// Entrypoint execution trapped or failed.
    #[error("failed to execute wasm entrypoint `{entrypoint}` in `{path}`: {source}")]
    WasmExecute {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Entrypoint export symbol name.
        entrypoint: String,
        /// Underlying Wasmtime execution error.
        source: wasmtime::Error,
    },
    /// Entrypoint reported domain-level desugarer failure.
    #[error("wasm desugarer returned failure for `{path}`: {message}")]
    WasmRuntimeFailure {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Error text read from desugarer error buffer.
        message: String,
    },
    /// Runtime output bytes were not valid UTF-8 text.
    #[error("failed to decode wasm output utf-8 for `{path}`: {source}")]
    OutputUtf8 {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// UTF-8 decode failure.
        source: std::str::Utf8Error,
    },
    /// Runtime output text was not valid `DesugarResponse` JSON.
    #[error("failed to parse wasm desugar response json for `{path}`: {source}")]
    OutputJson {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// JSON parse failure.
        source: serde_json::Error,
    },
    /// Desugar request could not be serialized to JSON.
    #[error("failed to serialize wasm desugar request json for `{path}`: {source}")]
    RequestJson {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// JSON serialize failure.
        source: serde_json::Error,
    },
    /// Desugarer returned an output variant this compiler version does not understand yet.
    #[error("desugarer returned an unsupported output variant for block keyword `{keyword}`")]
    UnsupportedOutput {
        /// Parsed keyword that introduced the failing block.
        keyword: String,
    },
    /// Expected pointer/length global export was missing.
    #[error("missing required wasm global `{global}` in `{path}`")]
    MissingWasmGlobal {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Expected global export name.
        global: String,
    },
    /// Pointer/length global was present but had an unexpected type.
    #[error("invalid wasm global `{global}` value in `{path}`")]
    InvalidWasmGlobal {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Global export name.
        global: String,
    },
    /// Pointer/length range exceeded module memory bounds.
    #[error("wasm output pointer/length out of bounds for `{path}`")]
    OutputBounds {
        /// Absolute path to the artifact file.
        path: PathBuf,
    },
    /// Input request bytes could not fit into the guest-declared request buffer.
    #[error("wasm input pointer/length out of bounds for `{path}`")]
    InputBounds {
        /// Absolute path to the artifact file.
        path: PathBuf,
    },
    /// A required mutable `i32` global could not be updated.
    #[error("wasm global `{global}` in `{path}` must be a mutable i32")]
    UnwritableWasmGlobal {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Global export name.
        global: String,
    },
    /// Wasmtime engine could not be configured/constructed.
    #[error("failed to initialize wasm engine: {0}")]
    EngineInit(String),
    /// Runtime memory-cell layout exposed by the desugarer was invalid after initialization.
    #[error("invalid wasm runtime layout for `{path}`: {message}")]
    InvalidRuntimeLayout {
        /// Absolute path to the artifact file.
        path: PathBuf,
        /// Human-readable layout validation detail.
        message: String,
    },
}
