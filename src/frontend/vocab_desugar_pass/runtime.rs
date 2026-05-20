use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, ExternType, Instance, Linker, Module, Store, Val, ValType};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p1::{self, WasiP1Ctx};

use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};

use super::VocabDesugarPassError;

const OUTPUT_PTR_GLOBAL: &str = incan_vocab::WASM_DESUGAR_OUTPUT_PTR_GLOBAL;
const OUTPUT_LEN_GLOBAL: &str = incan_vocab::WASM_DESUGAR_OUTPUT_LEN_GLOBAL;
const ERROR_PTR_GLOBAL: &str = incan_vocab::WASM_DESUGAR_ERROR_PTR_GLOBAL;
const ERROR_LEN_GLOBAL: &str = incan_vocab::WASM_DESUGAR_ERROR_LEN_GLOBAL;
const INPUT_PTR_GLOBAL: &str = incan_vocab::WASM_DESUGAR_INPUT_PTR_GLOBAL;
const INPUT_CAPACITY_GLOBAL: &str = incan_vocab::WASM_DESUGAR_INPUT_CAPACITY_GLOBAL;
const INPUT_LEN_GLOBAL: &str = incan_vocab::WASM_DESUGAR_INPUT_LEN_GLOBAL;
/// Required WASM export used to initialize buffer cells before `desugar_block()` runs.
const INIT_ENTRYPOINT: &str = incan_vocab::WASM_DESUGAR_INIT_ENTRYPOINT;
const MEMORY_EXPORT: &str = incan_vocab::WASM_DESUGAR_MEMORY_EXPORT;
const SUCCESS_STATUS: i32 = incan_vocab::WASM_DESUGAR_SUCCESS_STATUS;
/// Default fuel budget for one desugarer invocation.
///
/// The clean #455 nested companion repro traps at `250_000` units while the guest drops or serializes nested public AST
/// output. `5_000_000` admits that production-shaped path while still bounding accidental long-running desugarers.
/// If another legitimate companion desugarer hits this limit, raise it with a comment explaining the measured repro.
const DEFAULT_WASM_FUEL: u64 = 5_000_000;

/// Concrete Wasmtime store type used for one desugarer instantiation.
///
/// We keep the WASI context in the store because `wasm32-wasip1` Rust guests expect preview1 imports even when the
/// desugarer itself only exchanges data through the explicit ABI buffers.
type WasmStore = Store<WasiP1Ctx>;

/// Pointer/length values decoded from the guest runtime cells after initialization.
///
/// These are guest-memory offsets and sizes, not host pointers. Every value is validated against the module's exported
/// linear memory before the compiler reads or writes guest buffers.
#[derive(Debug, Clone, Copy)]
struct RuntimeLayoutValues {
    input_ptr: i32,
    input_capacity: i32,
    input_len: i32,
    output_ptr: i32,
    output_len: i32,
    error_ptr: i32,
    error_len: i32,
}

/// Fully resolved desugarer artifact metadata needed for one invocation.
#[derive(Debug, Clone)]
struct ResolvedWasmArtifact {
    path: PathBuf,
    expected_sha256: String,
    entrypoint: String,
    abi_version: u32,
}

/// Stateful runtime for loading and executing dependency-provided WASM desugarers.
pub struct WasmDesugarerRuntime {
    engine: Engine,
    /// Compiled modules keyed by artifact path. Populated on first use.
    modules: HashMap<PathBuf, Module>,
    /// Artifact paths whose SHA-256 has already been verified this compiler run.
    ///
    /// Once a module is compiled and cached, re-reading and re-hashing the file on every subsequent invocation is
    /// unnecessary I/O. The invariant is: every path in `modules` is also in `verified_artifacts`, so a cache hit
    /// implies prior integrity verification.
    verified_artifacts: HashSet<PathBuf>,
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
            verified_artifacts: HashSet::new(),
        })
    }

    /// Execute the dependency-provided desugarer for one bridged vocab syntax node.
    ///
    /// Modules are cached by artifact path so repeated DSL uses inside the same compiler run do not recompile the same
    /// guest module over and over.
    pub fn desugar_node(
        &mut self,
        library_manifest_index: &LibraryManifestIndex,
        node: &incan_vocab::VocabSyntaxNode,
        module_path: Option<&str>,
    ) -> Result<incan_vocab::DesugarResponse, VocabDesugarPassError> {
        let resolved = resolve_wasm_artifact_for_node(library_manifest_index, node)?;

        // ---- Context: compile and verify only on first encounter; reuse cache on subsequent calls ----
        let module = match self.modules.entry(resolved.path.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let artifact_path = entry.key().clone();
                let bytes = fs::read(&artifact_path).map_err(|source| VocabDesugarPassError::ArtifactRead {
                    path: artifact_path.clone(),
                    source,
                })?;
                verify_artifact_checksum(&artifact_path, &bytes, &resolved.expected_sha256)?;
                let compiled =
                    Module::new(&self.engine, &bytes).map_err(|source| VocabDesugarPassError::WasmCompile {
                        path: artifact_path.clone(),
                        source,
                    })?;
                self.verified_artifacts.insert(artifact_path);
                entry.insert(compiled)
            }
        };

        let request = incan_vocab::DesugarRequest {
            node: node.clone(),
            module_path: module_path.map(|value| value.to_string()),
        };

        execute_desugarer_module(&self.engine, module, &resolved, &request)
    }
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

    let artifact_path =
        resolve_desugarer_artifact_path(&metadata.crate_root, &desugarer_artifact.relative_path, keyword)?;
    Ok(ResolvedWasmArtifact {
        path: artifact_path,
        expected_sha256: desugarer_artifact.sha256.clone(),
        entrypoint: desugarer_artifact.entrypoint.clone(),
        abi_version: desugarer_artifact.abi_version,
    })
}

fn resolve_desugarer_artifact_path(
    crate_root: &Path,
    relative_path: &str,
    keyword: &str,
) -> Result<PathBuf, VocabDesugarPassError> {
    // ---- Context: reject obvious path escapes before touching the filesystem ----
    let candidate = Path::new(relative_path);
    if candidate.is_absolute() {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.to_string(),
            message: format!("desugarer artifact path `{relative_path}` must be relative to dependency artifact root"),
        });
    }
    if candidate.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.to_string(),
            message: format!("desugarer artifact path `{relative_path}` escapes dependency artifact root"),
        });
    }

    // ---- Context: canonicalize when possible for defense in depth ----
    let artifact_path = crate_root.join(candidate);
    if let (Ok(root), Ok(artifact)) = (crate_root.canonicalize(), artifact_path.canonicalize())
        && !artifact.starts_with(&root)
    {
        return Err(VocabDesugarPassError::Resolution {
            keyword: keyword.to_string(),
            message: format!("desugarer artifact path `{relative_path}` resolves outside dependency artifact root"),
        });
    }
    Ok(artifact_path)
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
/// - request/output/error buffers are exchanged through linear memory
/// - exported `__incan_*` globals identify 4-byte guest cells that store those buffer offsets and lengths
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

    // ---- Context: create one isolated guest store with bounded execution fuel ----
    let mut store = Store::new(engine, WasiCtxBuilder::new().build_p1());
    if store.set_fuel(DEFAULT_WASM_FUEL).is_err() {
        return Err(VocabDesugarPassError::WasmRuntimeFailure {
            path: resolved.path.clone(),
            message: "failed to set wasm fuel budget".to_string(),
        });
    }

    // ---- Context: validate and instantiate the canonical ABI surface ----
    validate_wasm_runtime_contract(module, &resolved.path, &resolved.entrypoint)?;

    let mut linker = Linker::new(engine);
    p1::add_to_linker_sync(&mut linker, |ctx| ctx).map_err(|source| VocabDesugarPassError::WasmInstantiate {
        path: resolved.path.clone(),
        source,
    })?;
    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|source| VocabDesugarPassError::WasmInstantiate {
            path: resolved.path.clone(),
            source,
        })?;
    let memory =
        instance
            .get_memory(&mut store, MEMORY_EXPORT)
            .ok_or_else(|| VocabDesugarPassError::MissingMemory {
                path: resolved.path.clone(),
            })?;
    initialize_desugarer_instance(&instance, &mut store, resolved)?;
    validate_initialized_runtime_layout(&instance, &memory, &mut store, resolved)?;
    write_request_json_payload(&instance, &memory, &mut store, resolved, request)?;
    let entrypoint = instance
        .get_typed_func::<(), i32>(&mut store, &resolved.entrypoint)
        .map_err(|_| VocabDesugarPassError::InvalidEntrypointSignature {
            path: resolved.path.clone(),
            entrypoint: resolved.entrypoint.clone(),
        })?;

    // ---- Context: execute the guest and decode either output or structured failure text ----
    let status = entrypoint
        .call(&mut store, ())
        .map_err(|source| VocabDesugarPassError::WasmExecute {
            path: resolved.path.clone(),
            entrypoint: resolved.entrypoint.clone(),
            source,
        })?;
    if status == SUCCESS_STATUS {
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
///
/// We do this against the compiled module before instantiation so producer mistakes fail early with deterministic
/// compiler diagnostics rather than late traps.
fn validate_wasm_runtime_contract(
    module: &Module,
    module_path: &Path,
    entrypoint: &str,
) -> Result<(), VocabDesugarPassError> {
    validate_memory_export(module, module_path)?;
    validate_entrypoint_export(module, module_path, entrypoint, Some(ValType::I32))?;
    validate_entrypoint_export(module, module_path, INIT_ENTRYPOINT, None)?;
    for &global_name in incan_vocab::WASM_DESUGAR_REQUIRED_I32_GLOBAL_EXPORTS {
        validate_i32_global_export(module, module_path, global_name)?;
    }
    Ok(())
}

/// Check that a module exports the canonical desugarer linear memory.
fn validate_memory_export(module: &Module, module_path: &Path) -> Result<(), VocabDesugarPassError> {
    let export = module
        .get_export(MEMORY_EXPORT)
        .ok_or_else(|| VocabDesugarPassError::MissingMemory {
            path: module_path.to_path_buf(),
        })?;
    if matches!(export, ExternType::Memory(_)) {
        Ok(())
    } else {
        Err(VocabDesugarPassError::MissingMemory {
            path: module_path.to_path_buf(),
        })
    }
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

/// Validate runtime cell layout after `__incan_init_desugarer()` has populated pointer/length cells.
///
/// `__incan_init_desugarer()` is just a required exported function, not a source-file convention. Its job is to
/// populate the guest-side bookkeeping cells that tell the host where the input/output/error buffers live in linear
/// memory.
fn validate_initialized_runtime_layout(
    instance: &Instance,
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
    resolved: &ResolvedWasmArtifact,
) -> Result<(), VocabDesugarPassError> {
    let layout = RuntimeLayoutValues {
        input_ptr: read_i32_global(instance, memory, store, &resolved.path, INPUT_PTR_GLOBAL)?,
        input_capacity: read_i32_global(instance, memory, store, &resolved.path, INPUT_CAPACITY_GLOBAL)?,
        input_len: read_i32_global(instance, memory, store, &resolved.path, INPUT_LEN_GLOBAL)?,
        output_ptr: read_i32_global(instance, memory, store, &resolved.path, OUTPUT_PTR_GLOBAL)?,
        output_len: read_i32_global(instance, memory, store, &resolved.path, OUTPUT_LEN_GLOBAL)?,
        error_ptr: read_i32_global(instance, memory, store, &resolved.path, ERROR_PTR_GLOBAL)?,
        error_len: read_i32_global(instance, memory, store, &resolved.path, ERROR_LEN_GLOBAL)?,
    };
    let memory_len = memory.data(&mut *store).len();
    validate_runtime_layout_values(&resolved.path, memory_len, layout)
}

/// Validate concrete pointer/length values read from runtime cells against guest memory size.
fn validate_runtime_layout_values(
    path: &Path,
    memory_len: usize,
    layout: RuntimeLayoutValues,
) -> Result<(), VocabDesugarPassError> {
    validate_memory_range(
        path,
        "input buffer",
        layout.input_ptr,
        layout.input_capacity,
        memory_len,
    )?;
    validate_memory_range(path, "output buffer", layout.output_ptr, layout.output_len, memory_len)?;
    validate_memory_range(path, "error buffer", layout.error_ptr, layout.error_len, memory_len)?;
    if layout.input_len < 0 {
        return Err(VocabDesugarPassError::InvalidRuntimeLayout {
            path: path.to_path_buf(),
            message: "input length cell must be non-negative after desugarer initialization".to_string(),
        });
    }
    if layout.input_len > layout.input_capacity {
        return Err(VocabDesugarPassError::InvalidRuntimeLayout {
            path: path.to_path_buf(),
            message: format!(
                "input length cell value {} exceeds input capacity {}",
                layout.input_len, layout.input_capacity
            ),
        });
    }
    Ok(())
}

/// Validate one guest memory range addressed by runtime layout cells.
///
/// These checks are part of the sandbox boundary: the compiler only touches guest memory ranges that are non-negative,
/// in-bounds, and internally consistent.
fn validate_memory_range(
    path: &Path,
    label: &str,
    start_i32: i32,
    len_i32: i32,
    memory_len: usize,
) -> Result<(), VocabDesugarPassError> {
    let start = non_negative_i32_to_usize(path, label, "start", start_i32)?;
    let len = non_negative_i32_to_usize(path, label, "length", len_i32)?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| VocabDesugarPassError::InvalidRuntimeLayout {
            path: path.to_path_buf(),
            message: format!("{label} range overflows usize bounds"),
        })?;
    if end > memory_len {
        return Err(VocabDesugarPassError::InvalidRuntimeLayout {
            path: path.to_path_buf(),
            message: format!("{label} range {start}..{end} exceeds guest memory size {memory_len}"),
        });
    }
    Ok(())
}

/// Convert one guest `i32` component into a host `usize` only after rejecting negative values.
fn non_negative_i32_to_usize(
    path: &Path,
    label: &str,
    component: &str,
    value: i32,
) -> Result<usize, VocabDesugarPassError> {
    usize::try_from(value).map_err(|_| VocabDesugarPassError::InvalidRuntimeLayout {
        path: path.to_path_buf(),
        message: format!("{label} {component} value {value} must be non-negative"),
    })
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
    let params_ok = func_ty.params().next().is_none();
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
///
/// The globals do not hold the payload directly. They identify 4-byte cells in guest memory whose contents are the
/// actual pointer/length values for the requested buffer.
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
    let ptr = usize::try_from(ptr).map_err(|_| VocabDesugarPassError::OutputBounds {
        path: resolved.path.clone(),
    })?;
    let len = usize::try_from(len).map_err(|_| VocabDesugarPassError::OutputBounds {
        path: resolved.path.clone(),
    })?;
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
///
/// The host writes JSON bytes into the guest input buffer, then updates the input-length cell so the desugarer knows
/// how many bytes of the buffer are meaningful.
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
    let ptr = usize::try_from(ptr).map_err(|_| VocabDesugarPassError::InputBounds {
        path: resolved.path.clone(),
    })?;
    let capacity = usize::try_from(capacity).map_err(|_| VocabDesugarPassError::InputBounds {
        path: resolved.path.clone(),
    })?;
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
///
/// The exported global names are stable ABI hooks. Their values point at 4-byte cells in linear memory, and the cell
/// contents are the runtime values the host actually cares about.
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
///
/// We update the cell contents rather than mutating the export itself. This keeps the ABI compatible with how Rust
/// companion crates expose statics in `wasm32-wasip1`.
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
///
/// The cell address comes from an exported ABI global; it is always interpreted as a guest-memory offset.
fn read_i32_memory_cell(
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
    path: &Path,
    global_name: &str,
    cell_addr: i32,
) -> Result<i32, VocabDesugarPassError> {
    let start = usize::try_from(cell_addr).map_err(|_| VocabDesugarPassError::InvalidWasmGlobal {
        path: path.to_path_buf(),
        global: global_name.to_string(),
    })?;
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
///
/// This is the only host write performed through the ABI after initialization: we copy the request length into the
/// guest's declared bookkeeping cell once the request bytes are in place.
fn write_i32_memory_cell(
    memory: &wasmtime::Memory,
    store: &mut WasmStore,
    path: &Path,
    global_name: &str,
    cell_addr: i32,
    value: i32,
) -> Result<(), VocabDesugarPassError> {
    let start = usize::try_from(cell_addr).map_err(|_| VocabDesugarPassError::UnwritableWasmGlobal {
        path: path.to_path_buf(),
        global: global_name.to_string(),
    })?;
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
/// The canonical format is `DesugarResponse`. We also accept legacy bare `DesugarOutput` JSON to keep older companion
/// artifacts working during the transition period.
///
/// TODO: remove the `DesugarOutput` fallback once all companion crates have been updated to emit
/// `DesugarResponse` wrappers (track in <https://github.com/dannymeijer/incan/issues/...>).
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn rejects_parent_directory_artifact_escape() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let err = match resolve_desugarer_artifact_path(root.path(), "../escape.wasm", "route") {
            Err(err) => err,
            Ok(_) => panic!("expected path traversal rejection"),
        };
        assert!(
            matches!(err, VocabDesugarPassError::Resolution { .. }),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn rejects_current_directory_artifact_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let err = match resolve_desugarer_artifact_path(root.path(), "./escape.wasm", "route") {
            Err(err) => err,
            Ok(_) => panic!("expected non-normalized relative path rejection"),
        };
        assert!(
            matches!(err, VocabDesugarPassError::Resolution { .. }),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn runtime_layout_validation_rejects_input_len_over_capacity() -> Result<(), Box<dyn std::error::Error>> {
        let layout = RuntimeLayoutValues {
            input_ptr: 0,
            input_capacity: 8,
            input_len: 9,
            output_ptr: 32,
            output_len: 0,
            error_ptr: 64,
            error_len: 0,
        };
        let err = match validate_runtime_layout_values(Path::new("mock.wasm"), 128, layout) {
            Err(err) => err,
            Ok(_) => panic!("expected invalid layout"),
        };
        assert!(
            matches!(err, VocabDesugarPassError::InvalidRuntimeLayout { .. }),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn runtime_layout_validation_rejects_out_of_bounds_ranges() -> Result<(), Box<dyn std::error::Error>> {
        let layout = RuntimeLayoutValues {
            input_ptr: 0,
            input_capacity: 8,
            input_len: 0,
            output_ptr: 120,
            output_len: 16,
            error_ptr: 64,
            error_len: 0,
        };
        let err = match validate_runtime_layout_values(Path::new("mock.wasm"), 128, layout) {
            Err(err) => err,
            Ok(_) => panic!("expected out-of-bounds layout"),
        };
        assert!(
            matches!(err, VocabDesugarPassError::InvalidRuntimeLayout { .. }),
            "unexpected error: {err}"
        );
        Ok(())
    }
}
