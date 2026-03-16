//! Post-parse desugaring pass for imported vocab block DSLs.
//!
//! This module provides:
//! - AST rewriting from raw `Statement::VocabBlock` nodes to ordinary statements
//! - sandboxed WASM desugarer loading/execution for dependency-provided artifacts
//! - deterministic diagnostics for bridge/runtime/deserialization failures

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, ExternType, Instance, Linker, Module, Store, Val, ValType};

use crate::frontend::ast;
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::frontend::vocab_ast_bridge::{
    VocabAstBridgeError, internal_vocab_block_to_public, public_statements_to_internal,
};

const OUTPUT_PTR_GLOBAL: &str = "__incan_output_ptr";
const OUTPUT_LEN_GLOBAL: &str = "__incan_output_len";
const ERROR_PTR_GLOBAL: &str = "__incan_error_ptr";
const ERROR_LEN_GLOBAL: &str = "__incan_error_len";
const INPUT_PTR_GLOBAL: &str = "__incan_input_ptr";
const INPUT_CAPACITY_GLOBAL: &str = "__incan_input_capacity";
const INPUT_LEN_GLOBAL: &str = "__incan_input_len";
const DEFAULT_WASM_FUEL: u64 = 250_000;

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
    /// Entrypoint export shape did not match `() -> i32`.
    #[error("invalid entrypoint signature for `{entrypoint}` in `{path}` (expected `() -> i32`)")]
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
    /// Desugarer returned an expression where the current compiler path expects statements.
    #[error(
        "desugarer returned expression output for block keyword `{keyword}`, but only statement output is currently supported"
    )]
    UnexpectedExpressionOutput {
        /// Parsed keyword that introduced the failing block.
        keyword: String,
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
}

#[derive(Debug, Clone)]
struct ResolvedWasmArtifact {
    path: PathBuf,
    expected_sha256: String,
    entrypoint: String,
}

/// Stateful runtime for loading and executing dependency-provided WASM desugarers.
pub struct WasmDesugarerRuntime {
    engine: Engine,
    modules: HashMap<PathBuf, Module>,
}

impl WasmDesugarerRuntime {
    /// Create a runtime with fuel metering enabled.
    ///
    /// Fuel is used to bound guest execution work and reduce runaway desugarer risk at compile time.
    pub fn new() -> Result<Self, VocabDesugarPassError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|err| VocabDesugarPassError::EngineInit(err.to_string()))?;
        Ok(Self {
            engine,
            modules: HashMap::new(),
        })
    }

    pub fn desugar_node(
        &mut self,
        library_manifest_index: &LibraryManifestIndex,
        node: &incan_vocab::VocabSyntaxNode,
        module_path: Option<&str>,
    ) -> Result<incan_vocab::DesugarResponse, VocabDesugarPassError> {
        let resolved = resolve_wasm_artifact_for_node(library_manifest_index, node)?;
        let bytes = fs::read(&resolved.path).map_err(|source| VocabDesugarPassError::ArtifactRead {
            path: resolved.path.clone(),
            source,
        })?;
        verify_artifact_checksum(&resolved.path, &bytes, &resolved.expected_sha256)?;

        let module = if let Some(existing) = self.modules.get(&resolved.path) {
            existing.clone()
        } else {
            let compiled = Module::new(&self.engine, &bytes).map_err(|source| VocabDesugarPassError::WasmCompile {
                path: resolved.path.clone(),
                source,
            })?;
            self.modules.insert(resolved.path.clone(), compiled.clone());
            compiled
        };

        let request = incan_vocab::DesugarRequest {
            node: node.clone(),
            module_path: module_path.map(|value| value.to_string()),
        };

        execute_desugarer_module(&self.engine, &module, &resolved, &request)
    }
}

/// Rewrite all raw vocab blocks in a parsed program before typechecking/lowering.
///
/// This pass is the hard boundary ensuring downstream phases operate only on ordinary compiler statements, never
/// `Statement::VocabBlock`.
///
/// # Errors
///
/// Returns compiler diagnostics when:
/// - AST bridge mapping fails,
/// - desugarer artifact resolution/loading fails,
/// - WASM runtime execution fails, or
/// - desugarer output cannot be decoded/mapped back into internal AST.
pub fn desugar_program_vocab_blocks(
    program: &mut ast::Program,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
) -> Result<(), Vec<CompileError>> {
    let mut runtime = match WasmDesugarerRuntime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            return Err(vec![CompileError::type_error(
                format!("failed to initialize vocab wasm runtime: {err}"),
                ast::Span::default(),
            )]);
        }
    };
    let mut errors = Vec::new();

    for declaration in &mut program.declarations {
        match &mut declaration.node {
            ast::Declaration::Function(function) => rewrite_statement_list(
                &mut function.body,
                module_path,
                library_manifest_index,
                &mut runtime,
                &mut errors,
            ),
            ast::Declaration::Model(model) => {
                for method in &mut model.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(body, module_path, library_manifest_index, &mut runtime, &mut errors);
                    }
                }
            }
            ast::Declaration::Class(class) => {
                for method in &mut class.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(body, module_path, library_manifest_index, &mut runtime, &mut errors);
                    }
                }
            }
            ast::Declaration::Trait(trait_decl) => {
                for method in &mut trait_decl.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(body, module_path, library_manifest_index, &mut runtime, &mut errors);
                    }
                }
            }
            ast::Declaration::Newtype(newtype_decl) => {
                for method in &mut newtype_decl.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(body, module_path, library_manifest_index, &mut runtime, &mut errors);
                    }
                }
            }
            _ => {}
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

fn rewrite_statement_list(
    statements: &mut Vec<ast::Spanned<ast::Statement>>,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    errors: &mut Vec<CompileError>,
) {
    let mut rewritten = Vec::new();

    for statement in statements.drain(..) {
        let span = statement.span;
        match statement.node {
            ast::Statement::VocabBlock(block) => {
                let bridged = internal_vocab_block_to_public(&block, span);
                let bridged = match bridged {
                    Ok(value) => value,
                    Err(source) => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::Bridge {
                                keyword: block.keyword.clone(),
                                source,
                            },
                            span,
                        ));
                        continue;
                    }
                };

                let request_node = incan_vocab::VocabSyntaxNode::Declaration(bridged.clone());
                let desugared = runtime.desugar_node(library_manifest_index, &request_node, module_path);
                let desugared = match desugared {
                    Ok(value) => value,
                    Err(err) => {
                        errors.push(error_from_pass_error(err, span));
                        continue;
                    }
                };

                let public_statements = match desugared.output {
                    incan_vocab::DesugarOutput::Statements(statements) => statements,
                    incan_vocab::DesugarOutput::Expression(_) => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::UnexpectedExpressionOutput {
                                keyword: bridged.keyword.clone(),
                            },
                            span,
                        ));
                        continue;
                    }
                    _ => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::UnsupportedOutput {
                                keyword: bridged.keyword.clone(),
                            },
                            span,
                        ));
                        continue;
                    }
                };

                let mut lowered = match public_statements_to_internal(&public_statements) {
                    Ok(stmts) => stmts,
                    Err(source) => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::Bridge {
                                keyword: bridged.keyword.clone(),
                                source,
                            },
                            span,
                        ));
                        continue;
                    }
                };
                for lowered_statement in &mut lowered {
                    lowered_statement.span = span;
                }
                rewrite_statement_list(&mut lowered, module_path, library_manifest_index, runtime, errors);
                rewritten.extend(lowered);
            }
            ast::Statement::If(mut if_stmt) => {
                rewrite_statement_list(
                    &mut if_stmt.then_body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    errors,
                );
                for (_, elif_body) in &mut if_stmt.elif_branches {
                    rewrite_statement_list(elif_body, module_path, library_manifest_index, runtime, errors);
                }
                if let Some(else_body) = if_stmt.else_body.as_mut() {
                    rewrite_statement_list(else_body, module_path, library_manifest_index, runtime, errors);
                }
                rewritten.push(ast::Spanned::new(ast::Statement::If(if_stmt), span));
            }
            ast::Statement::While(mut while_stmt) => {
                rewrite_statement_list(
                    &mut while_stmt.body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::While(while_stmt), span));
            }
            ast::Statement::For(mut for_stmt) => {
                rewrite_statement_list(&mut for_stmt.body, module_path, library_manifest_index, runtime, errors);
                rewritten.push(ast::Spanned::new(ast::Statement::For(for_stmt), span));
            }
            other => rewritten.push(ast::Spanned::new(other, span)),
        }
    }

    *statements = rewritten;
}

/// Resolve the concrete desugarer artifact for a vocab syntax node.
///
/// Routing is keyed by `VocabKeywordMetadata.dependency_key`, then resolved through the loaded dependency manifest
/// entry.
fn resolve_wasm_artifact_for_node(
    library_manifest_index: &LibraryManifestIndex,
    node: &incan_vocab::VocabSyntaxNode,
) -> Result<ResolvedWasmArtifact, VocabDesugarPassError> {
    let (keyword, metadata) = match node {
        incan_vocab::VocabSyntaxNode::Declaration(decl) => (&decl.keyword, decl.keyword_metadata.as_ref()),
        incan_vocab::VocabSyntaxNode::Clause(clause) => (&clause.keyword, None),
        incan_vocab::VocabSyntaxNode::Statement(_) | incan_vocab::VocabSyntaxNode::Expression(_) => {
            return Err(VocabDesugarPassError::Resolution {
                keyword: "<non-dsl-node>".to_string(),
                message: "cannot resolve desugarer artifact for non-declaration DSL node".to_string(),
            });
        }
        _ => {
            return Err(VocabDesugarPassError::Resolution {
                keyword: "<unsupported-dsl-node>".to_string(),
                message:
                    "cannot resolve desugarer artifact for an unsupported vocab syntax node in this compiler version"
                        .to_string(),
            });
        }
    };

    let dependency_key = metadata
        .map(|metadata| metadata.dependency_key.as_str())
        .unwrap_or_default();
    if dependency_key.is_empty() {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.clone(),
            message: "missing dependency key in vocab keyword metadata".to_string(),
        });
    }

    let Some(entry) = library_manifest_index.get(dependency_key) else {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.clone(),
            message: format!("unknown dependency key `{dependency_key}`"),
        });
    };

    let LibraryManifestIndexEntry::Loaded { manifest, metadata } = entry else {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.clone(),
            message: format!("dependency `{dependency_key}` is not in loaded state"),
        });
    };
    let Some(vocab) = manifest.vocab.as_ref() else {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.clone(),
            message: format!("dependency `{dependency_key}` has no vocab payload"),
        });
    };
    let Some(desugarer_artifact) = vocab.desugarer_artifact.as_ref() else {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.clone(),
            message: format!("dependency `{dependency_key}` has no desugarer artifact"),
        });
    };

    let artifact_path = metadata.crate_root.join(&desugarer_artifact.relative_path);
    Ok(ResolvedWasmArtifact {
        path: artifact_path,
        expected_sha256: desugarer_artifact.sha256.clone(),
        entrypoint: desugarer_artifact.entrypoint.clone(),
    })
}

/// Validate artifact integrity against `.incnlib`-declared SHA-256.
fn verify_artifact_checksum(path: &Path, bytes: &[u8], expected_sha256: &str) -> Result<(), VocabDesugarPassError> {
    let actual_sha256 = hex::encode(Sha256::digest(bytes));
    if actual_sha256 == expected_sha256 {
        Ok(())
    } else {
        Err(VocabDesugarPassError::ChecksumMismatch {
            path: path.to_path_buf(),
        })
    }
}

/// Instantiate and execute the desugarer module entrypoint.
///
/// Contract:
/// - entrypoint export has signature `() -> i32`
/// - `0` means success, non-zero means failure
/// - output/error payload pointers are read from known global exports
fn execute_desugarer_module(
    engine: &Engine,
    module: &Module,
    resolved: &ResolvedWasmArtifact,
    request: &incan_vocab::DesugarRequest,
) -> Result<incan_vocab::DesugarResponse, VocabDesugarPassError> {
    let mut store = Store::new(engine, ());
    if store.set_fuel(DEFAULT_WASM_FUEL).is_err() {
        return Err(VocabDesugarPassError::WasmRuntimeFailure {
            path: resolved.path.clone(),
            message: "failed to set wasm fuel budget".to_string(),
        });
    }

    let linker = Linker::new(engine);
    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|source| VocabDesugarPassError::WasmInstantiate {
            path: resolved.path.clone(),
            source,
        })?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| VocabDesugarPassError::MissingMemory {
            path: resolved.path.clone(),
        })?;
    validate_entrypoint_export(module, &resolved.path, &resolved.entrypoint)?;
    write_request_json_payload(&instance, &memory, &mut store, resolved, request)?;
    let entrypoint = instance
        .get_typed_func::<(), i32>(&mut store, &resolved.entrypoint)
        .map_err(|_| VocabDesugarPassError::InvalidEntrypointSignature {
            path: resolved.path.clone(),
            entrypoint: resolved.entrypoint.clone(),
        })?;

    let status = entrypoint
        .call(&mut store, ())
        .map_err(|source| VocabDesugarPassError::WasmExecute {
            path: resolved.path.clone(),
            entrypoint: resolved.entrypoint.clone(),
            source,
        })?;
    if status == 0 {
        let json_text = read_global_json_payload(
            &instance,
            &memory,
            &mut store,
            resolved,
            OUTPUT_PTR_GLOBAL,
            OUTPUT_LEN_GLOBAL,
        )?;
        return serde_json::from_str::<incan_vocab::DesugarResponse>(&json_text).map_err(|source| {
            VocabDesugarPassError::OutputJson {
                path: resolved.path.clone(),
                source,
            }
        });
    }

    let message = read_global_json_payload(
        &instance,
        &memory,
        &mut store,
        resolved,
        ERROR_PTR_GLOBAL,
        ERROR_LEN_GLOBAL,
    )
    .unwrap_or_else(|_| "desugarer execution failed".to_string());
    Err(VocabDesugarPassError::WasmRuntimeFailure {
        path: resolved.path.clone(),
        message,
    })
}

/// Check that a module exports the configured entrypoint as `() -> i32`.
fn validate_entrypoint_export(
    module: &Module,
    module_path: &Path,
    entrypoint: &str,
) -> Result<(), VocabDesugarPassError> {
    let export = module
        .get_export(entrypoint)
        .ok_or_else(|| VocabDesugarPassError::MissingEntrypoint {
            path: module_path.to_path_buf(),
            entrypoint: entrypoint.to_string(),
        })?;
    let ExternType::Func(func_ty) = export else {
        return Err(VocabDesugarPassError::InvalidEntrypointSignature {
            path: module_path.to_path_buf(),
            entrypoint: entrypoint.to_string(),
        });
    };
    let params_ok = func_ty.params().len() == 0;
    let mut results = func_ty.results();
    let result_ok = matches!(results.next(), Some(ValType::I32)) && results.next().is_none();
    if params_ok && result_ok {
        Ok(())
    } else {
        Err(VocabDesugarPassError::InvalidEntrypointSignature {
            path: module_path.to_path_buf(),
            entrypoint: entrypoint.to_string(),
        })
    }
}

/// Read a UTF-8 payload from memory using exported pointer/length globals.
fn read_global_json_payload(
    instance: &Instance,
    memory: &wasmtime::Memory,
    store: &mut Store<()>,
    resolved: &ResolvedWasmArtifact,
    ptr_global: &str,
    len_global: &str,
) -> Result<String, VocabDesugarPassError> {
    let ptr = read_i32_global(instance, store, &resolved.path, ptr_global)?;
    let len = read_i32_global(instance, store, &resolved.path, len_global)?;
    if ptr < 0 || len < 0 {
        return Err(VocabDesugarPassError::OutputBounds {
            path: resolved.path.clone(),
        });
    }
    let ptr = ptr as usize;
    let len = len as usize;
    let data = memory.data(store);
    let end = ptr.saturating_add(len);
    if end > data.len() {
        return Err(VocabDesugarPassError::OutputBounds {
            path: resolved.path.clone(),
        });
    }
    let bytes = &data[ptr..end];
    let text = std::str::from_utf8(bytes).map_err(|source| VocabDesugarPassError::OutputUtf8 {
        path: resolved.path.clone(),
        source,
    })?;
    Ok(text.to_string())
}

/// Serialize the request and copy it into the guest-declared input buffer.
fn write_request_json_payload(
    instance: &Instance,
    memory: &wasmtime::Memory,
    store: &mut Store<()>,
    resolved: &ResolvedWasmArtifact,
    request: &incan_vocab::DesugarRequest,
) -> Result<(), VocabDesugarPassError> {
    let request_json = serde_json::to_vec(request).map_err(|source| VocabDesugarPassError::RequestJson {
        path: resolved.path.clone(),
        source,
    })?;

    let ptr = read_i32_global(instance, store, &resolved.path, INPUT_PTR_GLOBAL)?;
    let capacity = read_i32_global(instance, store, &resolved.path, INPUT_CAPACITY_GLOBAL)?;
    if ptr < 0 || capacity < 0 {
        return Err(VocabDesugarPassError::InputBounds {
            path: resolved.path.clone(),
        });
    }

    let ptr = ptr as usize;
    let capacity = capacity as usize;
    let len_i32 = i32::try_from(request_json.len()).map_err(|_| VocabDesugarPassError::InputBounds {
        path: resolved.path.clone(),
    })?;
    let end = ptr.saturating_add(request_json.len());
    {
        let data = memory.data_mut(&mut *store);
        if request_json.len() > capacity || end > data.len() {
            return Err(VocabDesugarPassError::InputBounds {
                path: resolved.path.clone(),
            });
        }
        data[ptr..end].copy_from_slice(&request_json);
    }
    set_i32_global(instance, store, &resolved.path, INPUT_LEN_GLOBAL, len_i32)?;
    Ok(())
}

/// Read one `i32` global export value.
fn read_i32_global(
    instance: &Instance,
    store: &mut Store<()>,
    path: &Path,
    global_name: &str,
) -> Result<i32, VocabDesugarPassError> {
    let global =
        instance
            .get_global(&mut *store, global_name)
            .ok_or_else(|| VocabDesugarPassError::MissingWasmGlobal {
                path: path.to_path_buf(),
                global: global_name.to_string(),
            })?;
    match global.get(&mut *store) {
        Val::I32(value) => Ok(value),
        _ => Err(VocabDesugarPassError::InvalidWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        }),
    }
}

/// Update one mutable `i32` global export.
fn set_i32_global(
    instance: &Instance,
    store: &mut Store<()>,
    path: &Path,
    global_name: &str,
    value: i32,
) -> Result<(), VocabDesugarPassError> {
    let global =
        instance
            .get_global(&mut *store, global_name)
            .ok_or_else(|| VocabDesugarPassError::MissingWasmGlobal {
                path: path.to_path_buf(),
                global: global_name.to_string(),
            })?;
    global
        .set(&mut *store, Val::I32(value))
        .map_err(|_| VocabDesugarPassError::UnwritableWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        })
}

/// Map a pass/runtime error into a standard type-error diagnostic.
fn error_from_pass_error(error: VocabDesugarPassError, fallback_span: ast::Span) -> CompileError {
    CompileError::type_error(format!("vocab desugar pass failed: {error}"), fallback_span)
}
