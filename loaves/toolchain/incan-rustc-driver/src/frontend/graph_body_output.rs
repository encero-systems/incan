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
pub(super) struct PendingBodyValue {
    module: String,
    name: String,
    span: incan_frontend::ast::Span,
    identity: String,
    pub(super) declaration: Vec<u8>,
    pub(super) local: Vec<u8>,
    pub(super) final_value: Vec<u8>,
    body_ir: Vec<u8>,
    dependency_reads: Vec<internal_models::CompiledBodyDependencyRead>,
    /// Whether the checker restored this body from a selected record instead of checking its statements.
    restored: bool,
    restored_body_ir: bool,
    physical_inputs: Option<[u8; 32]>,
}

/// One complete namespace value awaiting its whole-module Body IR row.
struct PendingNamespaceValue {
    module: String,
    interface: Vec<u8>,
    semantic_interface: Vec<u8>,
    package_roots: Vec<String>,
    member_owners: std::collections::BTreeMap<incan_semantics_core::CanonicalSymbolId, String>,
    member_witnesses: Vec<Vec<String>>,
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
pub(super) struct BodyValueRefusal {
    pub(super) reason: String,
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
    /// Whether this invocation requested graph rows for freshly checked modules.
    pub(super) fn enabled(&self) -> bool {
        self.path.is_some()
    }

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
    pub(super) fn observe(
        &mut self,
        module: &ParsedModule,
        checker: &TypeChecker,
        dependencies: &[String],
        package_identity: &str,
    ) {
        if self.path.is_none() || self.error.is_some() {
            return;
        }
        let result = self.capture(module, checker, dependencies, package_identity);
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    /// Capture one project-owned module while keeping dependency modules as inputs rather than publication owners.
    ///
    /// Analysis visits authored modules and dependency modules together. Only paths beneath `project_root` belong to
    /// this publication; a dependency's checked bodies remain authenticated dependency facts and must never make a
    /// successful project build fail.
    fn capture(
        &mut self,
        module: &ParsedModule,
        checker: &TypeChecker,
        dependencies: &[String],
        package_identity: &str,
    ) -> Result<(), String> {
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
            let published = super::namespace_crumb::publish(&interface)?;
            let member_owners = interface.lowering_member_owners();
            super::namespace_crumb::remember_published(&published.interface, interface);
            let mut package_roots = incan_driver::body_ir::checked_package_roots(
                &[(module.path_segments.as_slice(), checker.type_info())],
                package_identity,
            )
            .iter()
            .map(incan_semantics_core::encode_incan_symbol_identity)
            .collect::<Vec<_>>();
            package_roots.sort_unstable();
            self.namespaces.push(PendingNamespaceValue {
                package_roots,
                module: module_path.clone(),
                interface: published.interface,
                semantic_interface: published.semantic_interface,
                member_owners,
                member_witnesses: Vec::new(),
                body_ir: Vec::new(),
                dependencies: dependencies.to_vec(),
                lowering_inputs: Vec::new(),
            });
        }
        for output in
            incan_frontend::checker_records::checked_graph_record_outputs(&module.ast, checker, &module.path_segments)?
        {
            match output {
                incan_frontend::checker_records::CheckedOwnerRecordOutput::Value(fields) => {
                    match encoded_body_value(&module_path, &fields) {
                        Ok(value) => self.values.push(PendingBodyValue {
                            physical_inputs: checker.graph_body_physical_inputs(&module.ast, fields.span),
                            restored: checker.restored_body_record(fields.span),
                            restored_body_ir: checker.type_info().selected_callable_body(fields.span).is_some(),
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
        body_ir: &mut [BodyIrModule],
        lowering_inputs: &[(incan_semantics_core::CompilerNodeId, String)],
    ) -> Result<(), String> {
        if self.path.is_none() {
            return Ok(());
        }
        if modules.len() != body_ir.len() {
            return Err("graph Body IR output is not aligned with authored modules".into());
        }
        let lowering: std::collections::HashMap<_, _> = lowering_inputs
            .iter()
            .map(|(owner, witness)| (owner, witness))
            .collect();
        for (module, lowered) in modules.iter().zip(body_ir) {
            let Ok(module_path) = module.file_path.strip_prefix(&self.project_root) else {
                continue;
            };
            let module_path = module_path.to_string_lossy().into_owned();
            if let Some(namespace) = self.namespaces.iter_mut().find(|value| value.module == module_path) {
                namespace.body_ir = incan_semantics_core::executable_representation::encode_namespace_module(lowered)
                    .map_err(|error| format!("Body IR namespace row output refused: {error}"))?;
                for body in &lowered.bodies {
                    let Some(canonical) = &body.canonical else { continue };
                    let Some(stable) = namespace.member_owners.get(canonical) else {
                        continue;
                    };
                    let Some(&witness) = lowering.get(&body.direct_call_id) else {
                        continue;
                    };
                    namespace.member_witnesses.push(vec![
                        incan_semantics_core::encode_incan_symbol_identity(canonical),
                        stable.clone(),
                        witness.clone(),
                    ]);
                }
                namespace.member_witnesses.sort();
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
                pending.body_ir = incan_semantics_core::executable_representation::encode_graph_owner(
                    &lowered.module_id,
                    &module_path,
                    &pending.identity,
                    body,
                    pending.physical_inputs,
                )
                .map_err(|error| format!("Body IR owner row output refused: {error}"))?;
            }
        }
        if let Some(value) = self.values.iter().find(|value| value.body_ir.is_empty()) {
            return Err(format!(
                "compiled graph output has no Body IR row for {} in {}",
                value.identity, value.module
            ));
        }
        if let Some(value) = self.namespaces.iter().find(|value| value.body_ir.is_empty()) {
            return Err(format!(
                "compiled graph output has no Body IR namespace row for {}",
                value.module
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
        self.namespaces.sort_by(|left, right| left.module.cmp(&right.module));
        let reusable_declarations = self.values.len();
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
        let restored = self
            .values
            .iter()
            .filter(|value| value.restored)
            .map(|value| value.identity.clone())
            .collect::<Vec<_>>();
        let report = internal_models::encode_checker_counts(&internal_models::CheckerRecordCounts {
            all_declarations: i64::try_from(all_declarations).map_err(|_| "checker declaration count exceeds i64")?,
            reusable_declarations: i64::try_from(reusable_declarations)
                .map_err(|_| "checker declaration count exceeds i64")?,
            summary,
            artifact_refusals: artifact_refusals
                .into_iter()
                .map(|(name, count)| i64::try_from(count).map(|count| (name, count)))
                .collect::<Result<_, _>>()
                .map_err(|_| "checker refusal count exceeds i64")?,
            restored,
            restored_body_ir: self
                .values
                .iter()
                .filter(|value| value.restored_body_ir)
                .map(|value| value.identity.clone())
                .collect(),
        })?;
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
        let namespaces: Vec<internal_models::CompiledNamespaceValue> = self
            .namespaces
            .into_iter()
            .map(|value| internal_models::CompiledNamespaceValue {
                module: value.module,
                interface: value.interface,
                semantic_interface: value.semantic_interface,
                member_witnesses: value.member_witnesses,
                package_roots: value.package_roots,
                body_ir: value.body_ir,
                dependencies: value.dependencies,
                lowering_inputs: value.lowering_inputs,
            })
            .collect();
        let body_count = values.len();
        let outputs = internal_models::CompiledBodyOutputs {
            namespaces,
            values,
            fallbacks,
        };
        let rows = internal_models::encode_compiled_body_rows(&outputs).map_err(|error| {
            format!(
                "compiled graph output refused: {error:?} ({body_count} owner rows, {body_ir_bytes} raw Body IR bytes)"
            )
        })?;
        publish_rows(&path, rows)?;
        publish(&checker_report_path(&path), vec![report])
    }
}

/// Convert one compiler-produced owner independently so a codec limitation cannot refuse its successful build.
pub(super) fn encoded_body_value(
    module: &str,
    fields: &incan_frontend::checker_records::CheckedOwnerRecordFields,
) -> Result<PendingBodyValue, BodyValueRefusal> {
    let declaration = internal_models::from_native_fields(&fields.declaration)
        .and_then(|record| internal_models::encode_record(&record))
        .map_err(|error| record_refusal("declaration", &fields.declaration, error))?;
    let local = internal_models::from_native_local_fields(&fields.local)
        .and_then(|record| internal_models::encode_record(&record))
        .map_err(|error| record_refusal("local", &fields.local, error))?;
    let final_value = internal_models::from_native_final_fields(&fields.final_value)
        .and_then(|record| internal_models::encode_record(&record))
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
        restored_body_ir: false,
        physical_inputs: None,
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
    let mut reason = format!("{stage} record output refused: {error:?}");
    if matches!(error, internal_models::CodecFault::Malformed) {
        // A bad field or an ordering violation invalidates every longer prefix. Ask the same schema to locate
        // the first bad row, so the refusal identifies its structural key without publishing its payload.
        let mut valid_end = 0;
        let mut end = rows.len() + 1;
        while end - valid_end > 1 {
            let middle = valid_end + (end - valid_end) / 2;
            let prefix = rows[..middle].to_vec();
            let valid = match stage {
                "declaration" => internal_models::from_native_fields(&prefix).is_ok(),
                "local" => internal_models::from_native_local_fields(&prefix).is_ok(),
                "final" => internal_models::from_native_final_fields(&prefix).is_ok(),
                _ => false,
            };
            if valid {
                valid_end = middle;
            } else {
                end = middle;
            }
        }
        if let Some(row) = rows.get(valid_end) {
            let key = row
                .iter()
                .take(3)
                .map(|part| part.chars().take(64).collect::<String>())
                .collect::<Vec<_>>()
                .join(" ");
            reason.push_str(&format!(" at row {valid_end} [{key}]"));
        }
    }
    BodyValueRefusal {
        reason,
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

/// Name the structured checker-admission report beside the graph body rows that produced it.
fn checker_report_path(path: &Path) -> PathBuf {
    let mut report = OsString::from(path.as_os_str());
    report.push(internal_models::checker_report_suffix());
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

/// Replace one compiler sidecar with a directory holding one independently bounded Crumb row per file.
fn publish_rows(path: &Path, rows: Vec<Vec<u8>>) -> Result<(), String> {
    let parent = path.parent().ok_or("compiled graph output has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create compiled graph output {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!("pending-{}", std::process::id()));
    std::fs::create_dir(&temporary)
        .map_err(|error| format!("cannot create compiled graph output {}: {error}", temporary.display()))?;
    for (index, row) in rows.into_iter().enumerate() {
        let page = temporary.join(format!("{index:08}.crumb"));
        std::fs::write(&page, row)
            .map_err(|error| format!("cannot write compiled graph output {}: {error}", page.display()))?;
    }
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
