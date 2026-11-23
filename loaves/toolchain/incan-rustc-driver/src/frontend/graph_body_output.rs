//! Bootstrap publication of compiler-produced checker records for Oven's ranked graph.
//!
//! The Incan codec owns validation and bytes. This adapter only collects field primitives at their existing checked
//! boundary, binds them to the source projection owner, and atomically writes the invocation-requested sidecar.

use std::collections::BTreeMap;
use std::ffi::OsString;
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
    /// Whether the checker restored this body from a selected record instead of checking its statements.
    restored: bool,
}

/// One complete namespace value awaiting its whole-module Body IR frame.
struct PendingNamespaceValue {
    module: String,
    interface: Vec<u8>,
    body_ir: Vec<u8>,
    dependencies: Vec<String>,
    lowering_inputs: Vec<Vec<String>>,
}

/// One checked owner excluded from record reuse while retaining the reason for the full-source path.
struct PendingBodyFallback {
    module: String,
    name: String,
    identity: Option<String>,
    reason: String,
    artifact_refusals: Vec<String>,
}

/// Why one compiler-produced owner could not be encoded, with the artifact fields whose rows the codec refused.
struct BodyValueRefusal {
    reason: String,
    artifact_refusals: Vec<String>,
}

/// Canonicalize the rendered owner inventory at the Rust-to-Incan framing boundary.
fn canonical_lowering_inputs(inputs: Vec<Vec<String>>) -> Result<Vec<Vec<String>>, String> {
    let mut canonical = std::collections::BTreeMap::new();
    for input in inputs {
        let [identity, witness] = input.as_slice() else {
            return Err("compiled graph output contains a malformed lowering witness".to_owned());
        };
        match canonical.entry(identity.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(witness.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() != witness => {
                return Err(format!(
                    "conflicting lowering witnesses for Body IR owner {}",
                    entry.key()
                ));
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    Ok(canonical
        .into_iter()
        .map(|(identity, witness)| vec![identity, witness])
        .collect())
}

/// Normalize an optional fallback owner before it crosses the Incan codec boundary.
fn fallback_identity(identity: Option<String>) -> Option<String> {
    identity.filter(|value| !value.is_empty())
}

/// Invocation-local collector for the optional graph-publication sidecar.
pub(super) struct GraphBodyOutput {
    path: Option<PathBuf>,
    project_root: PathBuf,
    values: Vec<PendingBodyValue>,
    namespaces: Vec<PendingNamespaceValue>,
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
            namespaces: Vec::new(),
            fallbacks: Vec::new(),
            error: None,
        })
    }

    /// Capture only values already emitted by all three producer boundaries for this checked module.
    pub(super) fn observe(&mut self, module: &ParsedModule, checker: &TypeChecker, dependencies: &[String]) {
        if self.path.is_none() || self.error.is_some() {
            return;
        }
        let result = self.capture(module, checker, dependencies);
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    /// Capture one project-owned module while keeping dependency modules as inputs rather than publication owners.
    ///
    /// Analysis visits authored modules and dependency modules together. Only paths beneath `project_root` belong to
    /// this publication; a dependency's checked bodies remain authenticated dependency facts and must never make a
    /// successful project build fail.
    fn capture(&mut self, module: &ParsedModule, checker: &TypeChecker, dependencies: &[String]) -> Result<(), String> {
        let Ok(module_path) = module.file_path.strip_prefix(&self.project_root) else {
            return Ok(());
        };
        let module_path = module_path.to_string_lossy().into_owned();
        if let Some(interface) = incan_frontend::typechecker::CheckedNamespaceInterface::from_checked(
            &module.name,
            &module.path_segments,
            &module.ast,
            checker,
        )? {
            self.namespaces.push(PendingNamespaceValue {
                module: module_path.clone(),
                interface: interface.to_bytes()?,
                body_ir: Vec::new(),
                dependencies: dependencies.to_vec(),
                lowering_inputs: Vec::new(),
            });
        }
        for output in
            incan_frontend::checker_records::checked_owner_record_outputs(&module.ast, checker, &module.path_segments)?
        {
            match output {
                incan_frontend::checker_records::CheckedOwnerRecordOutput::Value(fields) => {
                    match encoded_body_value(&module_path, &fields) {
                        Ok(value) => self.values.push(PendingBodyValue {
                            restored: checker.restored_body_record(fields.span),
                            ..value
                        }),
                        Err(refusal) => self.fallbacks.push(PendingBodyFallback {
                            module: module_path.clone(),
                            name: fields.name,
                            identity: fallback_identity(Some(fields.identity)),
                            reason: refusal.reason,
                            artifact_refusals: refusal.artifact_refusals,
                        }),
                    }
                }
                incan_frontend::checker_records::CheckedOwnerRecordOutput::Fallback(fallback) => {
                    self.fallbacks.push(PendingBodyFallback {
                        module: module_path.clone(),
                        name: fallback.name,
                        identity: fallback_identity(fallback.identity),
                        reason: fallback.reason,
                        artifact_refusals: fallback.artifact_refusals,
                    });
                }
            }
        }
        Ok(())
    }

    /// Attach the ordinary completed Body IR result to every eligible checker owner without projecting another
    /// semantic schema. The kernel codec retains the native `Body` contract and its module/source provenance.
    pub(super) fn attach_body_ir(
        &mut self,
        modules: &[ParsedModule],
        body_ir: &[BodyIrModule],
        lowering_inputs: &[(incan_semantics_core::CompilerNodeId, String)],
    ) -> Result<(), String> {
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
            if let Some(namespace) = self.namespaces.iter_mut().find(|value| value.module == module_path) {
                namespace.body_ir = incan_semantics_core::executable_representation::encode_module(lowered)
                    .map_err(|error| format!("Body IR namespace frame output refused: {error}"))?;
                namespace.lowering_inputs = canonical_lowering_inputs(
                    lowering_inputs
                        .iter()
                        .filter(|(identity, _)| lowered.bodies.iter().any(|body| &body.direct_call_id == identity))
                        .map(|(identity, witness)| vec![identity.path().to_owned(), witness.clone()])
                        .collect(),
                )?;
            }
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
        if let Some(value) = self.namespaces.iter().find(|value| value.body_ir.is_empty()) {
            return Err(format!(
                "compiled graph output has no Body IR namespace frame for {}",
                value.module
            ));
        }
        Ok(())
    }

    /// Frame every namespace, eligible triple and explicit fallback through the shared schema, then atomically
    /// publish them.
    ///
    /// Each output is framed on its own. An owner value the codec refuses joins the explicit fallbacks with that
    /// refusal, so it cannot refuse another owner's successful build; a refused namespace or fallback is named.
    pub(super) fn publish(mut self) -> Result<(), String> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };
        let _invocation = tracing::trace_span!(
            target: "incan::invocation",
            "incan_rustc_driver::publish_graph_body_output"
        )
        .entered();
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let (value_frames, restored, framed) = self.value_frames()?;
        let namespace_frames = self.namespace_frames()?;
        let fallback_frames = self.fallback_frames(&framed)?;
        let report = self.reuse_report(value_frames.len(), restored)?;
        let mut frames = namespace_frames;
        frames.extend(value_frames);
        frames.extend(fallback_frames);
        publish(&path, frames)?;
        publish(&checker_report_path(&path), vec![report])
    }

    /// Frame each eligible owner in canonical order, returning the frames, the restored owners among them, and each
    /// framed owner's module and identity in that order.
    ///
    /// A refused owner becomes a fallback that names the codec's refusal and is counted under [`VALUE_FRAME_REFUSAL`].
    fn value_frames(&mut self) -> Result<(Vec<Vec<u8>>, Vec<String>, Vec<(String, String)>), String> {
        let mut values = std::mem::take(&mut self.values);
        values.sort_by(|left, right| (&left.module, &left.identity).cmp(&(&right.module, &right.identity)));
        let mut frames = Vec::with_capacity(values.len());
        let mut framed = Vec::with_capacity(values.len());
        let mut restored = Vec::new();
        let mut previous: Option<(String, String)> = None;
        for value in values {
            let owner = (value.module.clone(), value.identity.clone());
            if previous.as_ref() == Some(&owner) {
                return Err(format!(
                    "compiled graph output has two owner values for {} in {}",
                    owner.1, owner.0
                ));
            }
            let name = value.name.clone();
            let was_restored = value.restored;
            let framed = internal_models::encode_compiled_body_outputs(internal_models::CompiledBodyOutputs {
                namespaces: Vec::new(),
                values: vec![internal_models::CompiledBodyValue {
                    module: value.module,
                    name: value.name,
                    identity: value.identity,
                    declaration: value.declaration,
                    local: value.local,
                    r#final: value.final_value,
                    body_ir: value.body_ir,
                    dependency_reads: value.dependency_reads,
                }],
                fallbacks: Vec::new(),
            });
            match framed {
                Ok(encoded) => {
                    frames.extend(encoded);
                    if was_restored {
                        restored.push(owner.1.clone());
                    }
                    framed.push(owner.clone());
                }
                Err(error) => self.fallbacks.push(PendingBodyFallback {
                    module: owner.0.clone(),
                    name,
                    identity: fallback_identity(Some(owner.1.clone())),
                    reason: format!("owner value frame output refused: {error:?}"),
                    artifact_refusals: vec![VALUE_FRAME_REFUSAL.to_owned()],
                }),
            }
            previous = Some(owner);
        }
        Ok((frames, restored, framed))
    }

    /// Frame each complete namespace value in module order, naming the module whose frame the codec refuses.
    fn namespace_frames(&mut self) -> Result<Vec<Vec<u8>>, String> {
        let mut namespaces = std::mem::take(&mut self.namespaces);
        namespaces.sort_by(|left, right| left.module.cmp(&right.module));
        let mut frames = Vec::with_capacity(namespaces.len());
        let mut previous: Option<String> = None;
        for value in namespaces {
            let module = value.module.clone();
            if previous.as_ref() == Some(&module) {
                return Err(format!("compiled graph output has two namespace values for {module}"));
            }
            let body_ir_bytes = value.body_ir.len();
            let framed = internal_models::encode_compiled_body_outputs(internal_models::CompiledBodyOutputs {
                namespaces: vec![internal_models::CompiledNamespaceValue {
                    module: value.module,
                    interface: value.interface,
                    body_ir: value.body_ir,
                    dependencies: value.dependencies,
                    lowering_inputs: value.lowering_inputs,
                }],
                values: Vec::new(),
                fallbacks: Vec::new(),
            })
            .map_err(|error| {
                format!(
                    "compiled graph output refused the namespace frame of {module}: {error:?} ({body_ir_bytes} raw \
                     Body IR bytes)"
                )
            })?;
            frames.extend(framed);
            previous = Some(module);
        }
        Ok(frames)
    }

    /// Frame each explicit fallback in canonical order, naming the owner whose frame the codec refuses.
    ///
    /// `framed` holds the owners that kept a value frame, in order; an owner is never both a value and a fallback.
    fn fallback_frames(&mut self, framed: &[(String, String)]) -> Result<Vec<Vec<u8>>, String> {
        self.fallbacks.sort_by(|left, right| {
            (&left.module, &left.name, &left.identity).cmp(&(&right.module, &right.name, &right.identity))
        });
        let mut frames = Vec::with_capacity(self.fallbacks.len());
        let mut previous: Option<(&str, &str, Option<&str>)> = None;
        for fallback in &self.fallbacks {
            let owner = (
                fallback.module.as_str(),
                fallback.name.as_str(),
                fallback.identity.as_deref(),
            );
            if previous == Some(owner) {
                return Err(format!(
                    "compiled graph output has two fallbacks for {} in {}",
                    fallback.name, fallback.module
                ));
            }
            if let Some(identity) = fallback.identity.as_deref() {
                let value = framed.binary_search_by(|(module, owner)| {
                    (module.as_str(), owner.as_str()).cmp(&(fallback.module.as_str(), identity))
                });
                if value.is_ok() {
                    return Err(format!(
                        "compiled graph output has both an owner value and a fallback for {identity} in {}",
                        fallback.module
                    ));
                }
            }
            let framed = internal_models::encode_compiled_body_outputs(internal_models::CompiledBodyOutputs {
                namespaces: Vec::new(),
                values: Vec::new(),
                fallbacks: vec![internal_models::CompiledBodyFallback {
                    module: fallback.module.clone(),
                    name: fallback.name.clone(),
                    identity: fallback.identity.clone(),
                    reason: fallback.reason.clone(),
                }],
            })
            .map_err(|error| {
                format!(
                    "compiled graph output refused the fallback frame of {} in {}: {error:?}",
                    fallback.name, fallback.module
                )
            })?;
            frames.extend(framed);
            previous = Some(owner);
        }
        Ok(frames)
    }

    /// Render the checker reuse report over the owners that kept their value frame and every explicit fallback.
    fn reuse_report(&self, reusable_declarations: usize, restored: Vec<String>) -> Result<Vec<u8>, String> {
        let all_declarations = reusable_declarations + self.fallbacks.len();
        let mut artifact_refusals = BTreeMap::<String, usize>::new();
        for fallback in &self.fallbacks {
            for refusal in &fallback.artifact_refusals {
                *artifact_refusals.entry(refusal.clone()).or_default() += 1;
            }
        }
        let refusal_text = artifact_refusals
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = if refusal_text.is_empty() {
            format!("{reusable_declarations}/{all_declarations} reusable declarations")
        } else {
            format!("{reusable_declarations}/{all_declarations} reusable declarations; refusals {refusal_text}")
        };
        // A selected record the checker refused is not restored: that body was checked afresh, and Oven counts it so.
        serde_json::to_vec(&serde_json::json!({
            "schema": "incan.checker-reuse/1",
            "summary": summary,
            "reusable_declarations": reusable_declarations,
            "all_declarations": all_declarations,
            "artifact_refusals": artifact_refusals,
            "restored": restored,
        }))
        .map_err(|error| format!("cannot encode checker reuse report: {error}"))
    }
}

/// The reuse-report name of an owner whose complete value frame the codec refused after its records were accepted.
const VALUE_FRAME_REFUSAL: &str = "owner-value-frame";

/// Convert one compiler-produced owner independently so a codec limitation cannot refuse its successful build.
fn encoded_body_value(
    module: &str,
    fields: &incan_frontend::checker_records::CheckedOwnerRecordFields,
) -> Result<PendingBodyValue, BodyValueRefusal> {
    let declaration = internal_models::from_native_fields(&fields.declaration)
        .and_then(internal_models::encode_record)
        .map_err(|error| record_refusal("declaration", &fields.declaration, error))?;
    let local = internal_models::from_native_local_fields(&fields.local)
        .and_then(internal_models::encode_record)
        .map_err(|error| record_refusal("local", &fields.local, error))?;
    let final_value = internal_models::from_native_final_fields(&fields.final_value)
        .and_then(internal_models::encode_record)
        .map_err(|error| record_refusal("final", &fields.final_value, error))?;
    let mut dependency_reads = fields
        .dependency_reads
        .iter()
        .map(|read| internal_models::CompiledBodyDependencyRead {
            package: read.package.clone(),
            module: dependency_module(&read.module_path),
            name: read.declaration_name.clone(),
            identity: read.declaration_identity.clone(),
            reference_subject: read.reference_subject.clone(),
        })
        .collect::<Vec<_>>();
    dependency_reads.sort_by(|left, right| {
        (
            &left.package,
            &left.module,
            &left.name,
            &left.identity,
            &left.reference_subject,
        )
            .cmp(&(
                &right.package,
                &right.module,
                &right.name,
                &right.identity,
                &right.reference_subject,
            ))
    });
    dependency_reads.dedup_by(|left, right| {
        left.package == right.package
            && left.module == right.module
            && left.name == right.name
            && left.identity == right.identity
            && left.reference_subject == right.reference_subject
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
        restored: false,
    })
}

/// Explain one record codec refusal. An unsupported row names its artifact field, so the checker reuse report groups
/// a field the record schema refuses beside the fields the frontend refuses.
fn record_refusal(stage: &str, rows: &[Vec<String>], error: internal_models::CodecFault) -> BodyValueRefusal {
    let mut artifact_refusals = Vec::new();
    if let internal_models::CodecFault::Unsupported(name) = &error {
        artifact_refusals = rows
            .iter()
            .filter(|row| row.first() == Some(name) || row.get(2) == Some(name))
            .filter_map(|row| incan_frontend::checker_records::record_row_artifact_field(row))
            .collect();
        artifact_refusals.sort();
        artifact_refusals.dedup();
    }
    BodyValueRefusal {
        reason: format!("{stage} record output refused: {error:?}"),
        artifact_refusals,
    }
}

/// Render the compiler's canonical package module path in the logical source-key spelling shared with Oven.
fn dependency_module(module_path: &[String]) -> String {
    match module_path {
        [] => "lib".to_owned(),
        [root] if root == "main" => "lib".to_owned(),
        _ => module_path.join("/"),
    }
}

/// Name the structured checker-admission report beside the graph body frames that produced it.
fn checker_report_path(path: &Path) -> PathBuf {
    let mut report = OsString::from(path.as_os_str());
    report.push(".checker-report.json");
    PathBuf::from(report)
}

/// Replace one compiler sidecar atomically so a failed invocation cannot leave a partially encoded output.
fn publish(path: &Path, frames: Vec<Vec<u8>>) -> Result<(), String> {
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
    for frame in frames {
        output
            .write_all(&frame)
            .map_err(|error| format!("cannot write compiled graph output {}: {error}", temporary.display()))?;
    }
    output
        .flush()
        .map_err(|error| format!("cannot flush compiled graph output {}: {error}", temporary.display()))?;
    drop(output);
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot publish compiled graph output {}: {error}", path.display()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::{GraphBodyOutput, canonical_lowering_inputs, fallback_identity};

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

    /// Framing coalesces equal owner evidence and names disagreement before the Incan codec sees it.
    #[test]
    fn lowering_inputs_are_unique_by_rendered_owner() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            canonical_lowering_inputs(vec![
                vec!["main#decl.0..12".to_owned(), "body:a".to_owned()],
                vec!["main#decl.0..12".to_owned(), "body:a".to_owned()],
            ])?,
            vec![vec!["main#decl.0..12".to_owned(), "body:a".to_owned()]]
        );
        assert_eq!(
            canonical_lowering_inputs(vec![
                vec!["main#decl.0..12".to_owned(), "body:a".to_owned()],
                vec!["main#decl.0..12".to_owned(), "body:b".to_owned()],
            ]),
            Err("conflicting lowering witnesses for Body IR owner main#decl.0..12".to_owned())
        );
        Ok(())
    }

    /// A checker fallback without an identity is encoded as absence rather than an invalid present-empty value.
    #[test]
    fn fallback_identity_normalizes_empty_presence() {
        assert_eq!(fallback_identity(Some(String::new())), None);
        assert_eq!(fallback_identity(None), None);
        assert_eq!(fallback_identity(Some("owner".to_owned())), Some("owner".to_owned()));
    }
}
