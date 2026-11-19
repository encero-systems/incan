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
    coordinates: Vec<(u32, u32)>,
}

/// One complete namespace value awaiting its whole-module Body IR row.
struct PendingNamespaceValue {
    module: String,
    interface: Vec<u8>,
    semantic_interface: Vec<u8>,
    identity_proof: Vec<u8>,
    package_roots: Vec<String>,
    member_owners: std::collections::BTreeMap<incan_semantics_core::CanonicalSymbolId, String>,
    member_witnesses: Vec<Vec<String>>,
    call_reads: Vec<internal_models::SettledCallRead>,
    call_contracts: Option<Vec<internal_models::SettledCallContract>>,
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
    method_reads: Option<Vec<Vec<String>>>,
    namespace_proofs: bool,
    owner_coordinates: bool,
    report_contracts: bool,
    import_closures: Option<BTreeMap<String, Vec<String>>>,
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

    pub(super) fn requests_contracts(&self) -> bool {
        self.report_contracts
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
        // The requesting Oven chooses its output profile. A predecessor still receives the rows it can decode.
        let namespace_proofs = path.as_ref().and_then(|path| path.file_name()).is_some_and(|name| {
            name.as_encoded_bytes()
                .ends_with(internal_models::namespace_proof_output_suffix().as_bytes())
        });
        let import_closures = path
            .as_ref()
            .and_then(|path| path.file_name())
            .filter(|name| {
                name.as_encoded_bytes()
                    .ends_with(internal_models::ranked_output_suffix().as_bytes())
            })
            .map(|_| BTreeMap::new());
        let method_reads = path
            .as_ref()
            .and_then(|path| path.file_name())
            .filter(|name| {
                name.as_encoded_bytes()
                    .ends_with(internal_models::method_lookup_suffix().as_bytes())
            })
            .map(|_| Vec::new());
        let owner_coordinates = path.as_ref().and_then(|path| path.file_name()).is_some_and(|name| {
            name.as_encoded_bytes()
                .ends_with(internal_models::coordinate_output_suffix().as_bytes())
        });
        Ok(Self {
            import_closures,
            method_reads,
            namespace_proofs,
            owner_coordinates,
            report_contracts: path.as_ref().and_then(|p| p.file_name()).is_some_and(|name| {
                name.as_encoded_bytes()
                    .ends_with(internal_models::contract_output_suffix().as_bytes())
            }),
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

    /// Complete checked modules' certificates with their suppliers, including suppliers restored from records.
    pub(super) fn include_restored_imports(
        &mut self,
        entry: &std::path::Path,
        indexes: &BTreeMap<PathBuf, usize>,
        dependencies: impl Fn(usize) -> Vec<String>,
    ) -> Result<(), String> {
        let Some(closures) = &mut self.import_closures else {
            return Ok(());
        };
        // The native interface also consumes the entry namespace when its checker was restored. Anchor its rank in
        // today's collected imports before filling in the predecessor closures needed to certify that entry.
        if let Ok(relative) = entry.strip_prefix(&self.project_root) {
            let module = relative.to_string_lossy();
            if !closures.contains_key(module.as_ref()) {
                let index = indexes
                    .get(entry)
                    .ok_or_else(|| format!("missing collected entry {}", entry.display()))?;
                closures.insert(module.into_owned(), dependencies(*index));
            }
        }
        let missing = closures
            .values()
            .flatten()
            .filter(|path| !closures.contains_key(*path))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        for module in missing {
            let index = indexes
                .get(&self.project_root.join(&module))
                .ok_or_else(|| format!("ranked import {module} has no collected source"))?;
            closures.insert(module, dependencies(*index));
        }
        Ok(())
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
        if let Some(closures) = &mut self.import_closures {
            closures.insert(module_path.clone(), dependencies.to_vec());
        }
        if let Some(interface) = incan_frontend::typechecker::CheckedNamespaceInterface::from_checked(
            &module.name,
            &module.path_segments,
            &module.ast,
            checker,
        )? {
            let published = super::namespace_crumb::publish(&interface)?;
            let member_owners = interface.lowering_member_owners();
            let identity_proof = super::namespace_crumb::remember_published(
                &module_path,
                &published.interface,
                interface,
                self.namespace_proofs,
            )?;
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
                identity_proof,
                interface: published.interface,
                semantic_interface: published.semantic_interface,
                member_owners,
                member_witnesses: Vec::new(),
                call_reads: Vec::new(),
                call_contracts: None,
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
                    let restored = checker.restored_body_record(fields.span);
                    let uses_methods = uses_method_dispatch(&fields);
                    let witnessed = !uses_methods
                        || self.method_reads.is_some()
                            && (restored || fields.method_lookups.as_ref().is_some_and(|rows| !rows.is_empty()));
                    let record = if witnessed {
                        encoded_body_value(&module_path, &fields)
                    } else {
                        Err(BodyValueRefusal {
                            reason: "source method lookup has no complete candidate membership evidence".into(),
                            artifact_refusals: vec!["CallArtifacts.resolved_method_calls".into()],
                        })
                    };
                    if uses_methods
                        && record.is_ok()
                        && let Some(reads) = &mut self.method_reads
                    {
                        if restored {
                            reads.push(vec![fields.identity.clone()]);
                        } else if let Some(lookups) = &fields.method_lookups {
                            for lookup in lookups {
                                let mut cells = vec![
                                    fields.identity.clone(),
                                    dependency_module(&lookup.module),
                                    lookup.receiver.clone(),
                                    lookup.method.clone(),
                                ];
                                cells.extend(lookup.candidates.iter().cloned());
                                reads.push(cells);
                            }
                        }
                    }
                    match record {
                        Ok(value) => {
                            let inputs = checker.graph_body_inputs(&module.ast, fields.span, self.owner_coordinates);
                            let (physical_inputs, coordinates) = match inputs {
                                Some((digest, coordinates)) => (Some(digest), coordinates),
                                None => (None, Vec::new()),
                            };
                            self.values.push(PendingBodyValue {
                                physical_inputs,
                                coordinates,
                                restored,
                                restored_body_ir: checker.type_info().selected_callable_body(fields.span).is_some(),
                                ..value
                            });
                        }
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
        call_reads: &std::collections::BTreeMap<String, Vec<incan_frontend::body_ir::SettledCallRead>>,
        access: Option<&incan_frontend::body_ir::SettledCallInputs>,
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
        let mut namespaces = std::collections::HashMap::with_capacity(self.namespaces.len());
        for namespace in &mut self.namespaces {
            if namespaces.insert(namespace.module.clone(), namespace).is_some() {
                return Err("multiple checked namespaces for one module".into());
            }
        }
        // Preserve capture order when generated owners share a source span.
        let mut owners = BTreeMap::<String, BTreeMap<_, Vec<usize>>>::new();
        for (index, value) in self.values.iter().enumerate() {
            owners
                .entry(value.module.clone())
                .or_default()
                .entry(value.span)
                .or_default()
                .push(index);
        }
        for (module, lowered) in modules.iter().zip(body_ir) {
            let Ok(module_path) = module.file_path.strip_prefix(&self.project_root) else {
                continue;
            };
            let module_path = module_path.to_string_lossy().into_owned();
            if let Some(namespace) = namespaces.remove(&module_path) {
                namespace.call_reads = call_reads
                    .get(&module_path)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .map(|read| internal_models::SettledCallRead {
                        reader: read.reader,
                        target: read.target,
                        digest: read.digest,
                    })
                    .collect();
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
                if let Some(access) = access {
                    let mut contracts = BTreeMap::new();
                    for body in &lowered.bodies {
                        let Some((target, digest)) = body.canonical.as_ref().and_then(|id| access.supplied_by(id))
                        else {
                            continue;
                        };
                        let owner = owners
                            .get(&module_path)
                            .and_then(|by_span| by_span.get(&body.span))
                            .and_then(|candidates| {
                                candidates
                                    .iter()
                                    .map(|&i| &self.values[i])
                                    .find(|v| v.name == body.name)
                            })
                            .map(|pending| pending.identity.clone())
                            .unwrap_or_default();
                        contracts.insert(
                            target.to_owned(),
                            internal_models::SettledCallContract {
                                target: target.to_owned(),
                                digest: digest.to_owned(),
                                reader: body.direct_call_id.path().to_owned(),
                                owner,
                            },
                        );
                    }
                    // A restored provider may have no namespace row in this transaction. Use the supplier's
                    // current settled parameters to authenticate the consumer's read across that boundary.
                    for read in &namespace.call_reads {
                        let digest = access.contract(&read.target).ok_or("missing settled call supplier")?;
                        if digest != read.digest {
                            return Err("executable call witness differs from its settled supplier".into());
                        }
                        contracts
                            .entry(read.target.clone())
                            .or_insert_with(|| internal_models::SettledCallContract {
                                target: read.target.clone(),
                                digest: digest.to_owned(),
                                reader: String::new(),
                                owner: String::new(),
                            });
                    }
                    namespace.call_contracts = Some(contracts.into_values().collect());
                }
                namespace.member_witnesses.sort();
                namespace.lowering_inputs = canonical_lowering_inputs(
                    lowered
                        .bodies
                        .iter()
                        .filter_map(|body| {
                            let witness = lowering.get(&body.direct_call_id)?;
                            Some(vec![body.direct_call_id.path().to_owned(), (*witness).clone()])
                        })
                        .collect(),
                )?;
            }
            let mut by_reader = BTreeMap::<&str, Vec<internal_models::SettledCallRead>>::new();
            if let Some(reads) = call_reads.get(&module_path) {
                for read in reads {
                    by_reader
                        .entry(&read.reader)
                        .or_default()
                        .push(internal_models::SettledCallRead {
                            reader: read.reader.clone(),
                            target: read.target.clone(),
                            digest: read.digest.clone(),
                        });
                }
            }
            let no_reads = Vec::new();
            let Some(module_owners) = owners.get(&module_path) else {
                continue;
            };
            for body in &mut lowered.bodies {
                let Some(index) = module_owners.get(&body.span).and_then(|candidates| {
                    candidates
                        .iter()
                        .copied()
                        .find(|&index| self.values[index].name == body.name)
                }) else {
                    continue;
                };
                let pending = &mut self.values[index];
                pending.body_ir = incan_semantics_core::executable_representation::encode_graph_owner(
                    &lowered.module_id,
                    &module_path,
                    &pending.identity,
                    body,
                    pending.physical_inputs,
                )
                .map_err(|error| format!("Body IR owner row output refused: {error}"))?;
                if pending.physical_inputs.is_some() && TypeChecker::graph_body_has_scalar_placement(body) {
                    incan_semantics_core::executable_representation::append_owner_coordinates(
                        &mut pending.body_ir,
                        &pending.coordinates,
                    );
                }
                if call_reads.contains_key(&module_path) && incan_frontend::body_ir::needs_call_witness(body) {
                    let reader = body.direct_call_id.path();
                    let reads = by_reader.get(reader).unwrap_or(&no_reads);
                    let witness = internal_models::encode_owner_call_reads(
                        pending.identity.clone(),
                        reader.to_owned(),
                        module_path.clone(),
                        reads,
                    )?;
                    pending.body_ir.extend_from_slice(&witness);
                }
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

    /// Keep newly evaluated owners inside this activation. The ordinary physical and settled-call guards are
    /// applied again. No provisional namespace, dependency receipt or partial output is published.
    pub(super) fn completed_owners(
        mut self,
        modules: &[ParsedModule],
        bodies: &mut [BodyIrModule],
        reads: &BTreeMap<String, Vec<incan_frontend::body_ir::SettledCallRead>>,
    ) -> Result<Vec<incan_frontend::checker_records::GraphBodyRecord>, String> {
        self.values
            .retain(|value| !value.restored && !value.restored_body_ir && value.physical_inputs.is_some());
        if self.values.is_empty() {
            return Ok(Vec::new());
        }
        self.namespaces.clear();
        self.attach_body_ir(modules, bodies, &[], reads, None)?;
        let profile = incan_frontend::checker_records::GRAPH_BODY_RECORD_PROFILE.to_owned();
        self.values
            .into_iter()
            .map(|value| {
                let fields = |bytes: &Vec<u8>| {
                    internal_models::decode_owned_record(bytes, &profile, &value.identity)
                        .and_then(|record| internal_models::native_fields(&record))
                        .map_err(|error| format!("current checker output refused: {error:?}"))
                };
                let frame = incan_semantics_core::executable_representation::decode_owner(&value.body_ir)
                    .map_err(|error| format!("current executable owner refused: {error}"))?;
                Ok(incan_frontend::checker_records::GraphBodyRecord {
                    local: fields(&value.local)?,
                    final_value: fields(&value.final_value)?,
                    identity: value.identity,
                    body_ir: Some(frame),
                })
            })
            .collect()
    }

    /// Encode every eligible triple and explicit fallback through the shared schema, then atomically publish it.
    pub(super) fn publish(
        mut self,
        selected: &incan_frontend::checker_records::GraphBodyRecords,
    ) -> Result<(), String> {
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
        for value in &mut self.values {
            if selected.checked_here(&value.identity) {
                value.restored = false;
                value.restored_body_ir = false;
            }
        }
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
                identity_proof: value.identity_proof,
                semantic_interface: value.semantic_interface,
                member_witnesses: value.member_witnesses,
                call_contracts: value.call_contracts,
                call_reads: value.call_reads,
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
        let import_order = self
            .import_closures
            .map(|closures| {
                let rows = closures
                    .into_iter()
                    .map(|(module, imports)| std::iter::once(module).chain(imports).collect())
                    .collect::<Vec<Vec<String>>>();
                internal_models::encode_import_closures(&rows)
            })
            .transpose()?;
        let methods = self
            .method_reads
            .map(|mut rows| {
                rows.sort_unstable();
                rows.dedup();
                internal_models::encode_method_lookups(&rows)
            })
            .transpose()?;
        publish_rows(&path, rows, import_order, methods)?;
        publish(&checker_report_path(&path), vec![report])
    }
}

/// Does either checker product require evidence of its receiver's method membership?
pub(super) fn uses_method_dispatch(fields: &incan_frontend::checker_records::CheckedOwnerRecordFields) -> bool {
    fields.final_value.iter().chain(&fields.local).any(|row| {
        row.first().is_some_and(|value| value == "artifact-call")
            && row.get(2).is_some_and(|value| value == "resolved_method_calls")
    })
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
        coordinates: Vec::new(),
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
fn publish_rows(
    path: &Path,
    rows: Vec<Vec<u8>>,
    import_order: Option<Vec<u8>>,
    methods: Option<Vec<u8>>,
) -> Result<(), String> {
    let parent = path.parent().ok_or("compiled graph output has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create compiled graph output {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!("pending-{}", std::process::id()));
    std::fs::create_dir(&temporary)
        .map_err(|error| format!("cannot create compiled graph output {}: {error}", temporary.display()))?;
    if let Some(bytes) = import_order {
        let page = temporary.join("import-closures.crumb");
        std::fs::write(&page, bytes)
            .map_err(|error| format!("cannot write compiler import closure {}: {error}", page.display()))?;
    }
    if let Some(bytes) = methods {
        let page = temporary.join("method-lookups.crumb");
        std::fs::write(&page, bytes).map_err(|error| format!("cannot write {}: {error}", page.display()))?;
    }
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
        assert!(!collector.namespace_proofs);
        let current_output = root.join(format!("units{}", internal_models::namespace_proof_output_suffix()));
        assert!(GraphBodyOutput::prepare(Some(current_output), actual.clone())?.namespace_proofs);
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
