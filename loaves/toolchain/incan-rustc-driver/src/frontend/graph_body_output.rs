//! Bootstrap publication of compiler-produced checker records for Oven's ranked graph.
//!
//! The Incan codec owns validation and bytes. This adapter only collects field primitives at their existing checked
//! boundary, binds them to the source projection owner, and atomically writes the invocation-requested sidecar.

use std::io::Write;
use std::path::{Path, PathBuf};

use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeChecker;

/// One output awaiting canonical ordering and Incan-owned encoding.
struct PendingBodyValue {
    module: String,
    name: String,
    identity: String,
    declaration: Vec<u8>,
    local: Vec<u8>,
    final_value: Vec<u8>,
}

/// Invocation-local collector for the optional graph-publication sidecar.
pub(super) struct GraphBodyOutput {
    path: Option<PathBuf>,
    project_root: PathBuf,
    values: Vec<PendingBodyValue>,
    error: Option<String>,
}

impl GraphBodyOutput {
    /// Prepare an inert collector unless Oven explicitly requested one output path.
    pub(super) fn prepare(path: Option<PathBuf>, project_root: PathBuf) -> Self {
        Self {
            path,
            project_root,
            values: Vec::new(),
            error: None,
        }
    }

    /// Capture only values already emitted by all three producer boundaries for this checked module.
    pub(super) fn observe(&mut self, module: &ParsedModule, checker: &TypeChecker) {
        if self.path.is_none() || self.error.is_some() {
            return;
        }
        let result = self.capture(module, checker);
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    /// Capture one module while keeping the analysis callback's infallible observer contract.
    fn capture(&mut self, module: &ParsedModule, checker: &TypeChecker) -> Result<(), String> {
        let module_path = module
            .file_path
            .strip_prefix(&self.project_root)
            .map_err(|_| "checked graph output module is outside the project root")?
            .to_string_lossy()
            .into_owned();
        for fields in incan_frontend::checker_records::checked_owner_record_fields(&module.ast, checker) {
            let identity = header_identity(&fields.local)?;
            let declaration = internal_models::from_native_fields(&fields.declaration)
                .and_then(internal_models::encode_record)
                .map_err(|error| format!("declaration record output refused: {error:?}"))?;
            let local = internal_models::from_native_local_fields(&fields.local)
                .and_then(internal_models::encode_record)
                .map_err(|error| format!("local record output refused: {error:?}"))?;
            let final_value = internal_models::from_native_final_fields(&fields.final_value)
                .and_then(internal_models::encode_record)
                .map_err(|error| format!("final record output refused: {error:?}"))?;
            self.values.push(PendingBodyValue {
                module: module_path.clone(),
                name: fields.name,
                identity,
                declaration,
                local,
                final_value,
            });
        }
        Ok(())
    }

    /// Encode every collected triple through the shared Incan schema and atomically publish the sidecar.
    pub(super) fn publish(mut self) -> Result<(), String> {
        let Some(path) = self.path else {
            return Ok(());
        };
        if let Some(error) = self.error {
            return Err(error);
        }
        self.values
            .sort_by(|left, right| (&left.module, &left.identity).cmp(&(&right.module, &right.identity)));
        let values: Vec<internal_models::CompiledBodyValue> = self
            .values
            .into_iter()
            .map(|value| internal_models::CompiledBodyValue {
                module: value.module,
                name: value.name,
                identity: value.identity,
                declaration: value.declaration,
                local: value.local,
                r#final: value.final_value,
            })
            .collect();
        let encoded = internal_models::encode_compiled_body_values(&values)
            .map_err(|error| format!("compiled graph output refused: {error:?}"))?;
        publish(&path, &encoded)
    }
}

/// Read the stable owner identity from the native field header without interpreting any semantic field.
fn header_identity(fields: &[Vec<String>]) -> Result<String, String> {
    fields
        .first()
        .filter(|header| header.first().is_some_and(|field| field == "header"))
        .and_then(|header| header.get(1))
        .filter(|identity| !identity.is_empty())
        .cloned()
        .ok_or_else(|| "checked graph output is missing its owner identity".to_owned())
}

/// Replace one sidecar atomically so a failed invocation cannot leave a partially encoded output.
fn publish(path: &Path, encoded: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("compiled graph output has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create compiled graph output {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!("pending-{}", std::process::id()));
    let mut output = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("cannot open compiled graph output {}: {error}", temporary.display()))?;
    output
        .write_all(encoded)
        .and_then(|()| output.flush())
        .map_err(|error| format!("cannot write compiled graph output {}: {error}", temporary.display()))?;
    drop(output);
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot publish compiled graph output {}: {error}", path.display()))
}
