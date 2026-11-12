//! Bootstrap publication of compiler-produced checker records for Oven's ranked graph.
//!
//! The Incan codec owns validation and bytes. This adapter only collects field primitives at their existing checked
//! boundary, binds them to the source projection owner, and atomically writes the invocation-requested sidecar.

use std::io::Write;
use std::path::{Path, PathBuf};

use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeChecker;
use incan_semantics_core::body_ir::BodyIrModule;

/// One output awaiting canonical ordering and Incan-owned encoding.
struct PendingBodyValue {
    module: String,
    name: String,
    span: incan_frontend::ast::Span,
    identity: String,
    declaration: Vec<u8>,
    local: Vec<u8>,
    final_value: Vec<u8>,
    body_ir: Vec<u8>,
    dependency_reads: Vec<internal_models::CompiledBodyDependencyRead>,
}

/// One checked owner excluded from record reuse while retaining the reason for the full-source path.
struct PendingBodyFallback {
    module: String,
    name: String,
    identity: Option<String>,
    reason: String,
}

/// Invocation-local collector for the optional graph-publication sidecar.
pub(super) struct GraphBodyOutput {
    path: Option<PathBuf>,
    project_root: PathBuf,
    values: Vec<PendingBodyValue>,
    fallbacks: Vec<PendingBodyFallback>,
    error: Option<String>,
}

impl GraphBodyOutput {
    /// Prepare an inert collector unless Oven explicitly requested one output path.
    pub(super) fn prepare(path: Option<PathBuf>, project_root: PathBuf) -> Result<Self, String> {
        let project_root = if path.is_some() {
            project_root.canonicalize().map_err(|error| {
                format!(
                    "cannot resolve graph body project root {}: {error}",
                    project_root.display()
                )
            })?
        } else {
            project_root
        };
        Ok(Self {
            path,
            project_root,
            values: Vec::new(),
            fallbacks: Vec::new(),
            error: None,
        })
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

    /// Capture one project-owned module while keeping dependency modules as inputs rather than publication owners.
    ///
    /// Analysis visits authored modules and dependency modules together. Only paths beneath `project_root` belong to
    /// this publication; a dependency's checked bodies remain authenticated dependency facts and must never make a
    /// successful project build fail.
    fn capture(&mut self, module: &ParsedModule, checker: &TypeChecker) -> Result<(), String> {
        let Ok(module_path) = module.file_path.strip_prefix(&self.project_root) else {
            return Ok(());
        };
        let module_path = module_path.to_string_lossy().into_owned();
        for output in
            incan_frontend::checker_records::checked_owner_record_outputs(&module.ast, checker, &module.path_segments)?
        {
            match output {
                incan_frontend::checker_records::CheckedOwnerRecordOutput::Value(fields) => {
                    match encoded_body_value(&module_path, &fields) {
                        Ok(value) => self.values.push(value),
                        Err(reason) => self.fallbacks.push(PendingBodyFallback {
                            module: module_path.clone(),
                            name: fields.name,
                            identity: Some(fields.identity),
                            reason,
                        }),
                    }
                }
                incan_frontend::checker_records::CheckedOwnerRecordOutput::Fallback(fallback) => {
                    self.fallbacks.push(PendingBodyFallback {
                        module: module_path.clone(),
                        name: fallback.name,
                        identity: fallback.identity,
                        reason: fallback.reason,
                    });
                }
            }
        }
        Ok(())
    }

    /// Attach the ordinary completed Body IR result to every eligible checker owner without projecting another
    /// semantic schema. The kernel codec retains the native `Body` contract and its module/source provenance.
    pub(super) fn attach_body_ir(&mut self, modules: &[ParsedModule], body_ir: &[BodyIrModule]) -> Result<(), String> {
        if self.path.is_none() {
            return Ok(());
        }
        if modules.len() != body_ir.len() {
            return Err("graph Body IR output is not aligned with authored modules".into());
        }
        for (module, lowered) in modules.iter().zip(body_ir) {
            let Ok(module_path) = module.file_path.strip_prefix(&self.project_root) else {
                continue;
            };
            let module_path = module_path.to_string_lossy().into_owned();
            for body in &lowered.bodies {
                let Some(pending) = self
                    .values
                    .iter_mut()
                    .find(|value| value.module == module_path && value.name == body.name && value.span == body.span)
                else {
                    continue;
                };
                pending.body_ir = incan_semantics_core::executable_representation::encode_owner(
                    &lowered.module_id,
                    &module_path,
                    &pending.identity,
                    body,
                )
                .map_err(|error| format!("Body IR owner frame output refused: {error}"))?;
            }
        }
        if let Some(value) = self.values.iter().find(|value| value.body_ir.is_empty()) {
            return Err(format!(
                "compiled graph output has no Body IR frame for {} in {}",
                value.identity, value.module
            ));
        }
        Ok(())
    }

    /// Encode every eligible triple and explicit fallback through the shared schema, then atomically publish it.
    pub(super) fn publish(mut self) -> Result<(), String> {
        let Some(path) = self.path else {
            return Ok(());
        };
        let _invocation = tracing::trace_span!(
            target: "incan::invocation",
            "incan_rustc_driver::publish_graph_body_output"
        )
        .entered();
        if let Some(error) = self.error {
            return Err(error);
        }
        self.values
            .sort_by(|left, right| (&left.module, &left.identity).cmp(&(&right.module, &right.identity)));
        self.fallbacks.sort_by(|left, right| {
            (&left.module, &left.name, &left.identity).cmp(&(&right.module, &right.name, &right.identity))
        });
        let body_ir_bytes: usize = self.values.iter().map(|value| value.body_ir.len()).sum();
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
                body_ir: value.body_ir,
                dependency_reads: value.dependency_reads,
            })
            .collect();
        let fallbacks: Vec<internal_models::CompiledBodyFallback> = self
            .fallbacks
            .into_iter()
            .map(|fallback| internal_models::CompiledBodyFallback {
                module: fallback.module,
                name: fallback.name,
                identity: fallback.identity,
                reason: fallback.reason,
            })
            .collect();
        let body_count = values.len();
        let outputs = internal_models::CompiledBodyOutputs { values, fallbacks };
        let encoded = internal_models::encode_compiled_body_outputs(outputs)
            .map_err(|error| {
                format!(
                    "compiled graph output refused: {error:?} ({body_count} owner frames, {body_ir_bytes} raw Body IR bytes)"
                )
            })?;
        publish(&path, &encoded)
    }
}

/// Convert one compiler-produced owner independently so a codec limitation cannot refuse its successful build.
fn encoded_body_value(
    module: &str,
    fields: &incan_frontend::checker_records::CheckedOwnerRecordFields,
) -> Result<PendingBodyValue, String> {
    let declaration = internal_models::from_native_fields(&fields.declaration)
        .and_then(internal_models::encode_record)
        .map_err(|error| format!("declaration record output refused: {error:?}"))?;
    let local = internal_models::from_native_local_fields(&fields.local)
        .and_then(internal_models::encode_record)
        .map_err(|error| format!("local record output refused: {error:?}"))?;
    let final_value = internal_models::from_native_final_fields(&fields.final_value)
        .and_then(internal_models::encode_record)
        .map_err(|error| format!("final record output refused: {error:?}"))?;
    let mut dependency_reads = fields
        .dependency_reads
        .iter()
        .map(|read| internal_models::CompiledBodyDependencyRead {
            package: read.package.clone(),
            module: dependency_module(&read.module_path),
            name: read.declaration_name.clone(),
            identity: read.declaration_identity.clone(),
        })
        .collect::<Vec<_>>();
    dependency_reads.sort_by(|left, right| {
        (&left.package, &left.module, &left.name, &left.identity).cmp(&(
            &right.package,
            &right.module,
            &right.name,
            &right.identity,
        ))
    });
    dependency_reads.dedup_by(|left, right| {
        left.package == right.package
            && left.module == right.module
            && left.name == right.name
            && left.identity == right.identity
    });
    Ok(PendingBodyValue {
        module: module.to_owned(),
        name: fields.name.clone(),
        span: fields.span,
        identity: fields.identity.clone(),
        declaration,
        local,
        final_value,
        body_ir: Vec::new(),
        dependency_reads,
    })
}

/// Render the compiler's canonical package module path in the logical source-key spelling shared with Oven.
fn dependency_module(module_path: &[String]) -> String {
    match module_path {
        [] => "lib".to_owned(),
        [root] if root == "main" => "lib".to_owned(),
        _ => module_path.join("/"),
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::GraphBodyOutput;

    /// Explicit sidecar roots resolve aliases and refuse missing roots; an inactive collector needs neither.
    #[test]
    fn sidecar_root_resolves_aliases_and_refuses_missing_roots() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("incan-graph-root-{}", std::process::id()));
        std::fs::create_dir(&root)?;
        let actual = root.join("actual");
        let alias = root.join("alias");
        std::fs::create_dir(&actual)?;
        std::os::unix::fs::symlink(&actual, &alias)?;
        let output = Some(root.join("bodies"));
        let collector = GraphBodyOutput::prepare(output.clone(), alias)?;
        assert_eq!(collector.project_root, actual.canonicalize()?);
        let missing = root.join("missing");
        let refused = match GraphBodyOutput::prepare(output, missing.clone()) {
            Ok(_) => return Err("an explicit missing graph body root was accepted".into()),
            Err(message) => message,
        };
        assert!(refused.contains("cannot resolve graph body project root"));
        assert!(GraphBodyOutput::prepare(None, missing).is_ok());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
