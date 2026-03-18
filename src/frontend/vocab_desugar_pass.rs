//! Post-parse desugaring pass for imported vocab block DSLs.
//!
//! This module provides:
//! - AST rewriting from raw `Statement::VocabBlock` nodes to ordinary statements
//! - sandboxed WASM desugarer loading/execution for dependency-provided artifacts
//! - deterministic diagnostics for bridge/runtime/deserialization failures

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, ExternType, Instance, Linker, Module, Store, Val, ValType};
use wasmtime_wasi::p2::WasiCtxBuilder;
use wasmtime_wasi::preview1::WasiP1Ctx;

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
/// Required WASM export used to initialize buffer cells before `desugar_block()` runs.
const INIT_ENTRYPOINT: &str = "__incan_init_desugarer";
const DEFAULT_WASM_FUEL: u64 = 250_000;

type WasmStore = Store<WasiP1Ctx>;

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
}

#[derive(Debug, Clone)]
struct ResolvedWasmArtifact {
    path: PathBuf,
    expected_sha256: String,
    entrypoint: String,
    abi_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HelperImportSpec {
    dependency_key: String,
    exported_name: String,
    alias: String,
}

#[derive(Debug, Default)]
struct HelperImportAccumulator {
    imports: BTreeMap<(String, String), HelperImportSpec>,
}

impl HelperImportAccumulator {
    fn register(&mut self, dependency_key: &str, exported_name: &str) -> String {
        let key = (dependency_key.to_string(), exported_name.to_string());
        let alias = helper_import_alias(dependency_key, exported_name);
        let spec = HelperImportSpec {
            dependency_key: dependency_key.to_string(),
            exported_name: exported_name.to_string(),
            alias: alias.clone(),
        };
        self.imports.entry(key).or_insert(spec);
        alias
    }

    fn import_declarations(&self) -> Vec<ast::Spanned<ast::Declaration>> {
        let mut declarations = Vec::new();
        for spec in self.imports.values() {
            declarations.push(ast::Spanned::new(
                ast::Declaration::Import(ast::ImportDecl {
                    visibility: ast::Visibility::Private,
                    kind: ast::ImportKind::PubFrom {
                        library: spec.dependency_key.clone(),
                        items: vec![ast::ImportItem {
                            name: spec.exported_name.clone(),
                            alias: Some(spec.alias.clone()),
                        }],
                    },
                    alias: None,
                }),
                ast::Span::default(),
            ));
        }
        declarations
    }
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
    let mut helper_imports = HelperImportAccumulator::default();

    for declaration in &mut program.declarations {
        match &mut declaration.node {
            ast::Declaration::Function(function) => rewrite_statement_list(
                &mut function.body,
                module_path,
                library_manifest_index,
                &mut runtime,
                &mut helper_imports,
                &mut errors,
            ),
            ast::Declaration::Model(model) => {
                for method in &mut model.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            ast::Declaration::Class(class) => {
                for method in &mut class.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            ast::Declaration::Trait(trait_decl) => {
                for method in &mut trait_decl.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            ast::Declaration::Newtype(newtype_decl) => {
                for method in &mut newtype_decl.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    if errors.is_empty() {
        inject_helper_imports(program, &helper_imports);
        Ok(())
    } else {
        Err(errors)
    }
}

fn rewrite_statement_list(
    statements: &mut Vec<ast::Spanned<ast::Statement>>,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
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
                    incan_vocab::DesugarOutput::Expression(expression) => {
                        vec![incan_vocab::IncanStatement::Expr(expression)]
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
                let mut public_statements = public_statements;
                if let Err(message) = resolve_helper_bindings_in_statements(
                    &mut public_statements,
                    bridged.keyword_metadata.as_ref(),
                    &bridged.keyword,
                    library_manifest_index,
                    helper_imports,
                ) {
                    errors.push(error_from_pass_error(
                        VocabDesugarPassError::HelperBinding {
                            keyword: bridged.keyword.clone(),
                            message,
                        },
                        span,
                    ));
                    continue;
                }

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
                rewrite_statement_list(
                    &mut lowered,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.extend(lowered);
            }
            ast::Statement::If(mut if_stmt) => {
                rewrite_statement_list(
                    &mut if_stmt.then_body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                for (_, elif_body) in &mut if_stmt.elif_branches {
                    rewrite_statement_list(
                        elif_body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                if let Some(else_body) = if_stmt.else_body.as_mut() {
                    rewrite_statement_list(
                        else_body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                rewritten.push(ast::Spanned::new(ast::Statement::If(if_stmt), span));
            }
            ast::Statement::While(mut while_stmt) => {
                rewrite_statement_list(
                    &mut while_stmt.body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::While(while_stmt), span));
            }
            ast::Statement::For(mut for_stmt) => {
                rewrite_statement_list(
                    &mut for_stmt.body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::For(for_stmt), span));
            }
            other => rewritten.push(ast::Spanned::new(other, span)),
        }
    }

    *statements = rewritten;
}

fn helper_import_alias(dependency_key: &str, exported_name: &str) -> String {
    let sanitize = |value: &str| {
        value
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
    };
    format!(
        "__incan_vocab_helper_{}_{}",
        sanitize(dependency_key),
        sanitize(exported_name)
    )
}

fn inject_helper_imports(program: &mut ast::Program, helper_imports: &HelperImportAccumulator) {
    let imports = helper_imports.import_declarations();
    if imports.is_empty() {
        return;
    }

    let mut insert_at = 0usize;
    while let Some(declaration) = program.declarations.get(insert_at) {
        match declaration.node {
            ast::Declaration::Docstring(_) | ast::Declaration::Import(_) => insert_at += 1,
            _ => break,
        }
    }
    program.declarations.splice(insert_at..insert_at, imports);
}

fn resolve_helper_bindings_in_statements(
    statements: &mut [incan_vocab::IncanStatement],
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    library_manifest_index: &LibraryManifestIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    for statement in statements {
        resolve_helper_bindings_in_statement(
            statement,
            keyword_metadata,
            keyword,
            library_manifest_index,
            helper_imports,
        )?;
    }
    Ok(())
}

fn resolve_helper_bindings_in_statement(
    statement: &mut incan_vocab::IncanStatement,
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    library_manifest_index: &LibraryManifestIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    match statement {
        incan_vocab::IncanStatement::Expr(expr) => {
            resolve_helper_bindings_in_expr(expr, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanStatement::Return(Some(expr))
        | incan_vocab::IncanStatement::Assign { value: expr, .. }
        | incan_vocab::IncanStatement::Let { value: expr, .. } => {
            resolve_helper_bindings_in_expr(expr, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_helper_bindings_in_expr(
                condition,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            resolve_helper_bindings_in_statements(
                then_body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            resolve_helper_bindings_in_statements(
                else_body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )
        }
        incan_vocab::IncanStatement::While { condition, body } => {
            resolve_helper_bindings_in_expr(
                condition,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            resolve_helper_bindings_in_statements(
                body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )
        }
        incan_vocab::IncanStatement::For { iter, body, .. } => {
            resolve_helper_bindings_in_expr(iter, keyword_metadata, keyword, library_manifest_index, helper_imports)?;
            resolve_helper_bindings_in_statements(
                body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )
        }
        incan_vocab::IncanStatement::Pass | incan_vocab::IncanStatement::Return(None) => Ok(()),
        _ => Ok(()),
    }
}

fn resolve_helper_bindings_in_expr(
    expr: &mut incan_vocab::IncanExpr,
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    library_manifest_index: &LibraryManifestIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    match expr {
        incan_vocab::IncanExpr::Helper(helper_key) => {
            let keyword_metadata = keyword_metadata.ok_or_else(|| {
                format!(
                    "keyword `{keyword}` does not carry provider metadata, so helper `{helper_key}` cannot be resolved"
                )
            })?;
            let helper_binding =
                resolve_helper_binding(library_manifest_index, &keyword_metadata.dependency_key, helper_key)?;
            let alias = helper_imports.register(&keyword_metadata.dependency_key, &helper_binding.exported_name);
            *expr = incan_vocab::IncanExpr::Name(alias);
            Ok(())
        }
        incan_vocab::IncanExpr::List(items) | incan_vocab::IncanExpr::Tuple(items) => {
            for item in items {
                resolve_helper_bindings_in_expr(
                    item,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Dict(entries) => {
            for (key_expr, value_expr) in entries {
                resolve_helper_bindings_in_expr(
                    key_expr,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
                resolve_helper_bindings_in_expr(
                    value_expr,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Binary(left, _, right) => {
            resolve_helper_bindings_in_expr(left, keyword_metadata, keyword, library_manifest_index, helper_imports)?;
            resolve_helper_bindings_in_expr(right, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanExpr::Unary(_, value) => {
            resolve_helper_bindings_in_expr(value, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanExpr::Call { callee, args } => {
            resolve_helper_bindings_in_expr(
                callee,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            for arg in args {
                resolve_helper_bindings_in_expr(
                    arg,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Field { object, .. } => resolve_helper_bindings_in_expr(
            object,
            keyword_metadata,
            keyword,
            library_manifest_index,
            helper_imports,
        ),
        _ => Ok(()),
    }
}

fn resolve_helper_binding<'a>(
    library_manifest_index: &'a LibraryManifestIndex,
    dependency_key: &str,
    helper_key: &str,
) -> Result<&'a incan_vocab::HelperBinding, String> {
    let Some(entry) = library_manifest_index.get(dependency_key) else {
        return Err(format!("provider `pub::{dependency_key}` is not loaded"));
    };
    let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry else {
        return Err(format!("provider `pub::{dependency_key}` failed to load"));
    };
    let Some(vocab) = manifest.vocab.as_ref() else {
        return Err(format!(
            "provider `pub::{dependency_key}` does not expose vocab metadata"
        ));
    };
    vocab
        .provider_manifest
        .helper_bindings
        .iter()
        .find(|binding| binding.key == helper_key)
        .ok_or_else(|| format!("provider `pub::{dependency_key}` does not bind helper `{helper_key}`"))
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
        abi_version: desugarer_artifact.abi_version,
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
    if resolved.abi_version > incan_vocab::WASM_DESUGAR_ABI_VERSION {
        return Err(VocabDesugarPassError::WasmRuntimeFailure {
            path: resolved.path.clone(),
            message: format!(
                "desugarer ABI version {} is newer than compiler-supported version {}",
                resolved.abi_version,
                incan_vocab::WASM_DESUGAR_ABI_VERSION
            ),
        });
    }

    let mut store = Store::new(engine, WasiCtxBuilder::new().inherit_stdio().build_p1());
    if store.set_fuel(DEFAULT_WASM_FUEL).is_err() {
        return Err(VocabDesugarPassError::WasmRuntimeFailure {
            path: resolved.path.clone(),
            message: "failed to set wasm fuel budget".to_string(),
        });
    }

    validate_wasm_runtime_contract(module, &resolved.path, &resolved.entrypoint)?;

    let mut linker = Linker::new(engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |ctx| ctx).map_err(|source| {
        VocabDesugarPassError::WasmInstantiate {
            path: resolved.path.clone(),
            source,
        }
    })?;
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
    initialize_desugarer_instance(&instance, &mut store, resolved)?;
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
        return parse_desugar_response_json(&resolved.path, &json_text);
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

/// Validate the exported ABI surface required by the compiler runtime.
fn validate_wasm_runtime_contract(
    module: &Module,
    module_path: &Path,
    entrypoint: &str,
) -> Result<(), VocabDesugarPassError> {
    validate_entrypoint_export(module, module_path, entrypoint, Some(ValType::I32))?;
    validate_entrypoint_export(module, module_path, INIT_ENTRYPOINT, None)?;
    for global_name in [
        INPUT_PTR_GLOBAL,
        INPUT_CAPACITY_GLOBAL,
        INPUT_LEN_GLOBAL,
        OUTPUT_PTR_GLOBAL,
        OUTPUT_LEN_GLOBAL,
        ERROR_PTR_GLOBAL,
        ERROR_LEN_GLOBAL,
    ] {
        validate_i32_global_export(module, module_path, global_name)?;
    }
    Ok(())
}

/// Run the required desugarer initialization hook before any guest-memory access.
fn initialize_desugarer_instance(
    instance: &Instance,
    store: &mut WasmStore,
    resolved: &ResolvedWasmArtifact,
) -> Result<(), VocabDesugarPassError> {
    let init = instance
        .get_typed_func::<(), ()>(&mut *store, INIT_ENTRYPOINT)
        .map_err(|_| VocabDesugarPassError::InvalidEntrypointSignature {
            path: resolved.path.clone(),
            entrypoint: INIT_ENTRYPOINT.to_string(),
        })?;
    init.call(&mut *store, ())
        .map_err(|source| VocabDesugarPassError::WasmExecute {
            path: resolved.path.clone(),
            entrypoint: INIT_ENTRYPOINT.to_string(),
            source,
        })?;
    Ok(())
}

/// Check that a module exports the configured function as `()` returning the expected value.
fn validate_entrypoint_export(
    module: &Module,
    module_path: &Path,
    entrypoint: &str,
    expected_result: Option<ValType>,
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
    let result_ok = match expected_result {
        Some(ValType::I32) => matches!(results.next(), Some(ValType::I32)) && results.next().is_none(),
        None => results.next().is_none(),
        Some(_) => false,
    };
    if params_ok && result_ok {
        Ok(())
    } else {
        Err(VocabDesugarPassError::InvalidEntrypointSignature {
            path: module_path.to_path_buf(),
            entrypoint: entrypoint.to_string(),
        })
    }
}

/// Check that a module exports one `i32` global used as a memory-cell address.
fn validate_i32_global_export(
    module: &Module,
    module_path: &Path,
    global_name: &str,
) -> Result<(), VocabDesugarPassError> {
    let export = module
        .get_export(global_name)
        .ok_or_else(|| VocabDesugarPassError::MissingWasmGlobal {
            path: module_path.to_path_buf(),
            global: global_name.to_string(),
        })?;
    let ExternType::Global(global_ty) = export else {
        return Err(VocabDesugarPassError::InvalidWasmGlobal {
            path: module_path.to_path_buf(),
            global: global_name.to_string(),
        });
    };
    if matches!(global_ty.content(), ValType::I32) {
        Ok(())
    } else {
        Err(VocabDesugarPassError::InvalidWasmGlobal {
            path: module_path.to_path_buf(),
            global: global_name.to_string(),
        })
    }
}

/// Read a UTF-8 payload from memory using exported pointer/length globals.
fn read_global_json_payload(
    instance: &Instance,
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
    resolved: &ResolvedWasmArtifact,
    ptr_global: &str,
    len_global: &str,
) -> Result<String, VocabDesugarPassError> {
    let ptr = read_i32_global(instance, memory, store, &resolved.path, ptr_global)?;
    let len = read_i32_global(instance, memory, store, &resolved.path, len_global)?;
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
    store: &mut WasmStore,
    resolved: &ResolvedWasmArtifact,
    request: &incan_vocab::DesugarRequest,
) -> Result<(), VocabDesugarPassError> {
    let request_json = serde_json::to_vec(request).map_err(|source| VocabDesugarPassError::RequestJson {
        path: resolved.path.clone(),
        source,
    })?;

    let ptr = read_i32_global(instance, memory, store, &resolved.path, INPUT_PTR_GLOBAL)?;
    let capacity = read_i32_global(instance, memory, store, &resolved.path, INPUT_CAPACITY_GLOBAL)?;
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
    set_i32_global(instance, memory, store, &resolved.path, INPUT_LEN_GLOBAL, len_i32)?;
    Ok(())
}

/// Read one `i32` runtime cell via its exported address global.
fn read_i32_global(
    instance: &Instance,
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
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
        Val::I32(cell_addr) => read_i32_memory_cell(memory, store, path, global_name, cell_addr),
        _ => Err(VocabDesugarPassError::InvalidWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        }),
    }
}

/// Update one runtime cell via its exported address global.
fn set_i32_global(
    instance: &Instance,
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
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
    let cell_addr = match global.get(&mut *store) {
        Val::I32(addr) => addr,
        _ => {
            return Err(VocabDesugarPassError::InvalidWasmGlobal {
                path: path.to_path_buf(),
                global: global_name.to_string(),
            });
        }
    };
    write_i32_memory_cell(memory, store, path, global_name, cell_addr, value)
}

/// Read one little-endian i32 from a guest memory cell address.
fn read_i32_memory_cell(
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
    path: &Path,
    global_name: &str,
    cell_addr: i32,
) -> Result<i32, VocabDesugarPassError> {
    if cell_addr < 0 {
        return Err(VocabDesugarPassError::InvalidWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        });
    }
    let start = cell_addr as usize;
    let end = start.saturating_add(4);
    let data = memory.data(&mut *store);
    if end > data.len() {
        return Err(VocabDesugarPassError::InvalidWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        });
    }
    let mut bytes = [0_u8; 4];
    bytes.copy_from_slice(&data[start..end]);
    Ok(i32::from_le_bytes(bytes))
}

/// Write one little-endian i32 to a guest memory cell address.
fn write_i32_memory_cell(
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
    path: &Path,
    global_name: &str,
    cell_addr: i32,
    value: i32,
) -> Result<(), VocabDesugarPassError> {
    if cell_addr < 0 {
        return Err(VocabDesugarPassError::UnwritableWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        });
    }
    let start = cell_addr as usize;
    let end = start.saturating_add(4);
    let data = memory.data_mut(&mut *store);
    if end > data.len() {
        return Err(VocabDesugarPassError::UnwritableWasmGlobal {
            path: path.to_path_buf(),
            global: global_name.to_string(),
        });
    }
    data[start..end].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Decode one desugar response payload from guest JSON.
///
/// The canonical format is `DesugarResponse`. We also accept legacy bare `DesugarOutput` JSON
/// to keep older companion artifacts working during the transition period.
fn parse_desugar_response_json(
    module_path: &Path,
    json_text: &str,
) -> Result<incan_vocab::DesugarResponse, VocabDesugarPassError> {
    match serde_json::from_str::<incan_vocab::DesugarResponse>(json_text) {
        Ok(response) => Ok(response),
        Err(primary_error) => match serde_json::from_str::<incan_vocab::DesugarOutput>(json_text) {
            Ok(output) => Ok(incan_vocab::DesugarResponse { output }),
            Err(_) => Err(VocabDesugarPassError::OutputJson {
                path: module_path.to_path_buf(),
                source: primary_error,
            }),
        },
    }
}

/// Map a pass/runtime error into a standard type-error diagnostic.
fn error_from_pass_error(error: VocabDesugarPassError, fallback_span: ast::Span) -> CompileError {
    CompileError::type_error(format!("vocab desugar pass failed: {error}"), fallback_span)
}
