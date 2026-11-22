//! Checked program assembly: runtime declarations, checked modules, entrypoints, dependency contexts and retained
//! native layouts.

use super::*;

/// Recover generated-member ownership retained by the checker records that supplied reusable bodies.
///
/// A generated member has no independent executable representation: its checked nominal owner is the dependency
/// root. Conflicting owners for one canonical member are invalid retained checker evidence.
fn retained_generated_member_owners(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
) -> Result<BTreeMap<CanonicalSymbolId, CanonicalSymbolId>, String> {
    let mut owners = BTreeMap::new();
    for module in modules {
        let info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        for (member, owner) in &info.calls.compiler_generated_member_identities {
            if let Some(previous) = owners.insert(member.clone(), owner.clone())
                && previous != *owner
            {
                return Err(format!(
                    "compiler-generated member `{}` has conflicting retained owners",
                    member.render_compact(),
                ));
            }
        }
    }
    Ok(owners)
}

/// Recover nonlocal call roots from accepted Body IR when a reused checker record no longer carries the reference
/// edge that originally selected the body. The canonical target is producer evidence; source spelling is not.
fn retained_body_call_requirements(
    modules: &[BodyIrModule],
    package_identity: &str,
    generated_member_owners: &BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
) -> BTreeSet<CanonicalSymbolId> {
    let mut requirements = BTreeSet::new();
    for body in modules.iter().flat_map(|module| &module.bodies) {
        collect_retained_body_calls(
            &body.block.stmts,
            package_identity,
            generated_member_owners,
            &mut requirements,
        );
    }
    requirements
}

fn collect_retained_body_calls(
    statements: &[incan_semantics_core::body_ir::Statement],
    package_identity: &str,
    generated_member_owners: &BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
    requirements: &mut BTreeSet<CanonicalSymbolId>,
) {
    use incan_semantics_core::body_ir::{CallableTarget, Callee, Rvalue, StatementKind};

    for statement in statements {
        match &statement.kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } => {
                if let Some(identity) = &target.canonical {
                    let requirement = generated_member_owners.get(identity).unwrap_or(identity);
                    if matches!(&requirement.origin, SymbolOrigin::Package { library, .. } if library != package_identity)
                    {
                        requirements.insert(requirement.clone());
                    }
                }
            }
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    collect_retained_body_calls(
                        &arm.guard_stmts,
                        package_identity,
                        generated_member_owners,
                        requirements,
                    );
                    collect_retained_body_calls(
                        &arm.body_stmts,
                        package_identity,
                        generated_member_owners,
                        requirements,
                    );
                }
            }
            StatementKind::Assign {
                rvalue: Rvalue::Closure { body, .. },
                ..
            } => collect_retained_body_calls(&body.stmts, package_identity, generated_member_owners, requirements),
            StatementKind::Assign {
                rvalue: Rvalue::Generator { body, .. },
                ..
            } => collect_retained_body_calls(&body.stmts, package_identity, generated_member_owners, requirements),
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_retained_body_calls(
                    &then_block.stmts,
                    package_identity,
                    generated_member_owners,
                    requirements,
                );
                if let Some(block) = else_block {
                    collect_retained_body_calls(&block.stmts, package_identity, generated_member_owners, requirements);
                }
            }
            StatementKind::Loop { body } | StatementKind::UnsafeRegion { body, .. } => {
                collect_retained_body_calls(&body.stmts, package_identity, generated_member_owners, requirements);
            }
            StatementKind::Race { arms, .. } => {
                for arm in arms {
                    collect_retained_body_calls(
                        &arm.body.stmts,
                        package_identity,
                        generated_member_owners,
                        requirements,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Carry checker-selected native nominal identities into governed source bodies before publication drops held layouts.
/// Consumers recover the layouts through the ordinary held-Rust-items boundary; publication retains only used
/// identities.
pub(super) fn retain_native_type_identities(modules: &mut [BodyIrModule], native: &[BodyIrModule]) {
    let identities = native
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .filter(|declaration| matches!(declaration.canonical.origin, SymbolOrigin::RustCrate(_)))
        .map(|declaration| (declaration.name.clone(), declaration.canonical.clone()))
        .collect::<BTreeMap<_, _>>();
    extend_shared_identities(
        modules
            .iter_mut()
            .flat_map(|module| module.bodies.iter_mut())
            .map(|body| &mut body.named_type_identities),
        &identities,
    );
}

/// Add each missing identity to the bodies' shared identity maps. A map that already names every identity is kept,
/// and the maps that were one shared map stay one shared extension: this runs again on every runtime resolution, and
/// a private copy per body of a module-wide map multiplied the checked program.
fn extend_shared_identities<'a>(
    maps: impl Iterator<Item = &'a mut incan_semantics_core::body_ir::SharedIdentities>,
    identities: &BTreeMap<String, CanonicalSymbolId>,
) {
    // Keyed by the shared map's address; holding the original keeps that address from being reused while it is a key.
    let mut extended = std::collections::HashMap::new();
    for map in maps {
        if identities.keys().all(|name| map.contains_key(name)) {
            continue;
        }
        // A map no other body or copy holds is extended in place.
        if let Some(owned) = std::sync::Arc::get_mut(map) {
            for (name, identity) in identities {
                owned.entry(name.clone()).or_insert_with(|| identity.clone());
            }
            continue;
        }
        let (_, shared) = extended.entry(std::sync::Arc::as_ptr(map)).or_insert_with(|| {
            let mut copy = map.as_ref().clone();
            for (name, identity) in identities {
                copy.entry(name.clone()).or_insert_with(|| identity.clone());
            }
            (std::sync::Arc::clone(map), std::sync::Arc::new(copy))
        });
        *map = std::sync::Arc::clone(shared);
    }
}

/// Retain canonical declarations for calls introduced by native lowering.
///
/// A bootstrap Rust runtime has no Loaf entry and contributes no governed bodies. A native runtime enters through
/// Oven's ordinary artifact index, which reads its runtime bindings record by name; their identities select the same
/// `.incnbody` records as source imports. A lowering-provider producer uses its own checked export map, so
/// specializing its source needs no external artifact.
pub(super) fn runtime_declarations(
    session: &CompilationSession,
    local: Option<&LibraryManifest>,
) -> Result<
    (
        Vec<incan_semantics_core::body_ir::RuntimeCallableBinding>,
        Vec<(String, Vec<String>)>,
    ),
    String,
> {
    let mut bindings = Vec::new();
    let mut scopes = Vec::new();
    for package in ["incan_native_runtime", "incan_stdlib_primitives"] {
        if let Some(local) = local.filter(|manifest| manifest.name == package) {
            bindings.extend(incan_frontend::library_manifest::executable_surface::runtime_bindings(
                local,
            ));
            continue;
        }
        if session.loaves().can_query_runtime_bindings(package) {
            scopes.push((package.to_owned(), Vec::new()));
            continue;
        }
        let published = session
            .loaves()
            .runtime_bindings(package)
            .map_err(|error| error.to_string())?;
        if let Some(published) = published {
            bindings.extend(published.iter().cloned());
        }
    }
    Ok((bindings, scopes))
}

/// Collect a test unit's modules again with each checked file read from `sources`: the test file with its conftests
/// and its generated case entries (`crate::test_cases`), and in a batch the other modules and hosting conftests. The
/// test module keeps its logical identity, as ordinary collection gives it; outside the source root, a batch's entry
/// module is spelled from the batch's frame, as the modules it imports are (`crate::test_batch`).
fn collect_test_entries(
    path: &Path,
    sources: Vec<(PathBuf, String)>,
    frame: Option<&Path>,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let segments = incan_frontend::module::logical_module_segments_from_file(&session.source_root, path)
        .or_else(|| frame.and_then(|frame| incan_frontend::module::logical_module_segments_from_file(frame, path)))
        .unwrap_or_else(|| vec!["main".to_string()]);
    let overlay: std::collections::HashMap<PathBuf, String> = sources.into_iter().collect();
    incan_driver::modules::collect_document_modules(path.to_path_buf(), session, segments, &overlay)
}

/// Merge lowering witnesses from independent accepted products while refusing disagreement for one compiler owner.
fn canonical_lowering_inputs(
    inputs: Vec<(incan_semantics_core::CompilerNodeId, String)>,
) -> Result<Vec<(incan_semantics_core::CompilerNodeId, String)>, String> {
    let mut canonical = BTreeMap::new();
    for (identity, witness) in inputs {
        match canonical.entry(identity) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(witness);
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() != &witness => {
                return Err(format!(
                    "conflicting lowering witnesses for Body IR owner {}",
                    entry.key().path()
                ));
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    Ok(canonical.into_iter().collect())
}

/// Bind newly admitted lowering inputs to their call contracts. Keep the same ordered digest across cold publication
/// and legacy namespace admission, then carry it as long as every contract matches the current program.
pub(super) fn settle_lowering_witnesses<'a>(
    inputs: &mut [(incan_semantics_core::CompilerNodeId, String)],
    reads: impl Iterator<Item = &'a incan_frontend::body_ir::SettledCallRead>,
) -> Result<usize, String> {
    if inputs.is_empty() {
        return Ok(0);
    }
    let mut by_reader: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    {
        let wanted = inputs
            .iter()
            .map(|(id, _)| id.path())
            .collect::<std::collections::HashSet<_>>();
        for read in reads {
            if wanted.contains(read.reader.as_str()) {
                by_reader
                    .entry(&read.reader)
                    .or_default()
                    .push((&read.target, &read.digest));
            }
        }
    }
    let mut settled = 0;
    for (id, witness) in inputs {
        if let Some(reads) = by_reader.get(id.path()) {
            let digest = incan_semantics_core::semantic_digest::semantic_digest(reads).map_err(|e| e.to_string())?;
            let base = witness
                .split_once("|incan.settled-calls/1:")
                .map_or(witness.as_str(), |(base, _)| base);
            *witness = format!("{base}|incan.settled-calls/1:{digest}");
            settled += 1;
        }
    }
    Ok(settled)
}

/// Collect and analyze the described unit's source through its compilation session, retaining canonical checked facts.
///
/// Native-only declaration refusals follow analysis; desugaring and the Loaves the program imports from are owned by
/// the session. The declarations the program reaches in its Loaves (DD-0007) come from their `.incnbody` records, and
/// every module lowers against them. Private checked re-export calls and implicit newtype coercions add their exact
/// package targets to the request: their selected owners and hooks need not appear as written source references.
/// The unit's package is its registry package. Every source declaration, including an uncalled method, seeds
/// dependency discovery because all source bodies compile.
///
/// A library unit collects every authored module, checks each with its package identity and builds its manifest
/// from the same pass. Bodies with function or nominal-owner type parameters remain governed; other linked
/// callables name their compiled items.
///
/// A refusal by source diagnostics or by declaration validation is certified through `receipt`. With `inspection` the
/// manifest describes the source package for inspection (private registries, named by their source modules).
pub(super) fn checked_modules(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    resident: bool,
    receipt: &RefusalReceipt,
    mut graph_body_records: incan_frontend::checker_records::GraphBodyRecords,
    graph_body_output: Option<PathBuf>,
    collect_codegraph: bool,
    inspection: bool,
) -> Result<CheckedProgram, String> {
    loop {
        match checked_attempt(
            unit,
            rust,
            resident,
            receipt,
            &graph_body_records,
            graph_body_output.clone(),
            collect_codegraph,
            inspection,
        )? {
            CheckedAttempt::Complete(program) => return Ok(program),
            CheckedAttempt::Reopen(changes, current) => {
                graph_body_records.reopen_callers(changes)?;
                graph_body_records.retain_current(current);
            }
        }
    }
}

enum CheckedAttempt {
    Complete(CheckedProgram),
    Reopen(
        BTreeMap<String, BTreeSet<String>>,
        Vec<incan_frontend::checker_records::GraphBodyRecord>,
    ),
}

/// Check the selected program and collect the current imports, physical owners and Body IR for this attempt.
fn checked_attempt(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    resident: bool,
    receipt: &RefusalReceipt,
    graph_body_records: &incan_frontend::checker_records::GraphBodyRecords,
    graph_body_output: Option<PathBuf>,
    collect_codegraph: bool,
    inspection: bool,
) -> Result<CheckedAttempt, String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_modules", input = %unit.source.display()).entered();
    let path = unit
        .source
        .canonicalize()
        .map_err(|error| format!("cannot resolve source {}: {error}", unit.source.display()))?;
    // The resident process already keeps immutable readings. Its full canonical index needs no on-disk lookup
    // footprint, whose directory census would otherwise be repeated before and after every source edit.
    let session = if resident {
        CompilationSession::new(unit)
    } else {
        CompilationSession::for_native_program(unit)
    };
    let mut session = session
        .and_then(|session| session.with_rust_facts(rust))
        .map_err(|error| error.to_string())?;
    session.set_graph_body_records(graph_body_records.clone());
    let library = unit.kind == UnitKind::Library;
    let mut modules = if library {
        collect_library_modules_detailed_with_session(path.clone(), &session)
    } else {
        collect_modules_detailed_with_session(path.clone(), &session)
    }
    .map_err(|failure| receipt.diagnostic(failure))?;
    session
        .resolve_rust_facts(&modules)
        .map_err(|error| error.to_string())?;
    let collected = crate::test_unit::collect(unit, &session)?;
    // A test file whose tests take fixtures or parameters is checked with its generated case entries, and rustc reads
    // their spans from the same text; so is every module and hosting conftest of a test batch.
    crate::spans::retain_checked_sources(&collected.sources)?;
    if collected.sources.iter().any(|(file, _)| *file == path) {
        modules = collect_test_entries(&path, collected.sources, collected.frame.as_deref(), &session)
            .map_err(|failure| receipt.diagnostic(failure))?;
        session
            .resolve_rust_facts(&modules)
            .map_err(|error| error.to_string())?;
    }
    let tests = collected.tests;
    let package_identity = unit.package.as_str();
    let mut callable_products = callable_products::CallableProducts::prepare(unit, &mut session);
    let observes_graph = graph_body_output.is_some();
    let mut graph_body_output =
        graph_body_output::GraphBodyOutput::prepare(graph_body_output, unit.project_root.clone())?;
    let mut publication = library.then(|| PublicationFacts::new(package_identity).for_inspection(inspection));
    let imports = std::sync::Arc::new(incan_driver::modules::SourceModuleImports::new(
        &modules,
        session.own_standard_package(),
    ));
    let namespace_indexes = graph_body_output.enabled().then(|| {
        modules
            .iter()
            .enumerate()
            .map(|(index, module)| (module.file_path.clone(), index))
            .collect::<BTreeMap<_, _>>()
    });
    let project_dependencies = |index: usize| {
        let mut dependencies = imports
            .dependencies(index)
            .iter()
            .filter_map(|&dependency| {
                modules[dependency]
                    .file_path
                    .strip_prefix(&unit.project_root)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        dependencies.sort();
        dependencies.dedup();
        dependencies
    };
    let contextual_rank = graph_body_records.has_contextual_rank();
    let mut analysis = {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::analyze_modules", modules = modules.len()).entered();
        session
            .analyze_unit_with_imports(
                &modules,
                library.then_some(package_identity),
                std::sync::Arc::clone(&imports),
                library && collect_codegraph,
                &mut |module, checker, provider| {
                    if contextual_rank {
                        graph_body_records.count_contextual_statements(checker.check_work().body_local_checks);
                    }
                    callable_products.observe(module, checker);
                    if let Some(indexes) = &namespace_indexes
                        && let Some(&index) = indexes.get(module.file_path.as_path())
                    {
                        let dependencies = project_dependencies(index);
                        graph_body_output.observe(module, checker, provider, &dependencies, package_identity);
                    }
                    if let Some(publication) = publication.as_mut() {
                        publication.visit(module, checker, module.file_path == path);
                    }
                },
            )
            .map_err(|failure| receipt.diagnostic(failure))?
    };
    if let Some(indexes) = &namespace_indexes {
        graph_body_output.include_restored_imports(&path, indexes, project_dependencies)?;
    }
    let entry = modules
        .iter()
        .position(|module| module.file_path == path)
        .ok_or("entry module is missing")?;
    validate_module_declarations(&modules, &analysis).inspect_err(|_| receipt.deterministic())?;
    let body_ir_inputs_invocation = tracing::trace_span!(
        target: "incan::invocation",
        "incan_rustc_driver::prepare_body_ir_inputs"
    )
    .entered();
    let manifest = match publication {
        Some(publication) => {
            let manifest = publication
                .library_manifest(&modules, &modules[entry], unit, &session.contract_model_bundles)
                .map_err(|error| error.to_string())?;
            Some(manifest)
        }
        None => None,
    };
    let mut requested = required_loaf_identities(&modules, &analysis, package_identity)?;
    let checked_local = checked_local_identities(manifest.as_ref());
    let (runtime_callable_bindings, runtime_binding_scopes) = runtime_declarations(&session, manifest.as_ref())?;
    let resolved = load_loaf_modules(session.loaves(), &requested, &checked_local)?;
    drop(body_ir_inputs_invocation);

    // ---- Declarations the program reaches in its Loaves ----
    let mut package_versions = resolved.package_versions;
    package_versions.insert(unit.package.clone(), unit.version.clone());
    let mut reached_loaves = resolved.entries;
    let mut bodies = resolved.modules;
    // Loaf declarations retain canonical spans but carry no source text. Do not attribute those spans to the entry.
    let mut files = bodies
        .iter()
        .map(|module| module.module_id.path().to_owned())
        .collect::<Vec<_>>();
    let mut sources = vec![String::new(); bodies.len()];
    let mut loaf_modules = bodies.len();
    let mut entry = entry + loaf_modules;

    // ---- The program's own modules ----
    let restored = modules
        .iter()
        .filter(|m| analysis.namespace_body_for_path(&m.file_path).is_some())
        .filter_map(|m| m.file_path.strip_prefix(&unit.project_root).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    let rebound = lower_authored_modules(&modules, &mut analysis, &session, &mut bodies, &mut files)?;
    let retained_generated_member_owners = retained_generated_member_owners(&modules, &analysis)?;
    let retained_requirements = retained_body_call_requirements(
        &bodies[loaf_modules..],
        package_identity,
        &retained_generated_member_owners,
    );
    if !retained_requirements
        .iter()
        .all(|identity| requested.contains(identity))
    {
        requested.extend(retained_requirements);
        let resolved = load_loaf_modules(session.loaves(), &requested, &checked_local)?;
        package_versions.extend(resolved.package_versions);
        reached_loaves = resolved.entries;
        let mut authored = bodies.split_off(loaf_modules);
        let authored_files = files.split_off(loaf_modules);
        entry = entry - loaf_modules + resolved.modules.len();
        bodies = resolved.modules;
        loaf_modules = bodies.len();
        files = bodies
            .iter()
            .map(|module| module.module_id.path().to_owned())
            .chain(authored_files)
            .collect();
        sources = vec![String::new(); loaf_modules];
        bodies.append(&mut authored);
    }
    let string_conversion_contract = incan_frontend::body_ir::compilation_string_conversion_contract(
        modules.iter().map(|module| &module.ast),
        session.loaves(),
        &requested,
    )
    .ok_or("unsupported Body IR string conversion contract identity")?;
    let mut reads_by_module = BTreeMap::new();
    let mut settled_inputs = None;
    if observes_graph || !graph_body_records.is_empty() {
        let access = incan_frontend::body_ir::SettledCallInputs::from_modules(&bodies[loaf_modules..])?;
        let mut changed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (module, lowered) in modules.iter().zip(&mut bodies[loaf_modules..]) {
            let Ok(relative) = module.file_path.strip_prefix(&unit.project_root) else {
                continue;
            };
            let source = relative.to_string_lossy().into_owned();
            let reads = if restored.contains(&source) {
                let reads = graph_body_records.settled_call_reads(&source)?;
                for read in &reads {
                    if !access.matches(read) {
                        changed.entry(source.clone()).or_default().insert(read.reader.clone());
                    }
                }
                reads
            } else {
                for read in graph_body_records.selected_call_reads(&source)? {
                    if !access.matches(&read) {
                        changed.entry(source.clone()).or_default().insert(read.reader);
                    }
                }
                access.reads(lowered)
            };
            reads_by_module.insert(source, reads);
        }
        if !changed.is_empty() {
            let current =
                graph_body_output.completed_owners(&modules, &mut bodies[loaf_modules..], &reads_by_module)?;
            return Ok(CheckedAttempt::Reopen(changed, current));
        }
        if graph_body_output.requests_contracts() {
            settled_inputs = Some(access);
        }
    }
    let mut cacheable_bodies = callable_products.publish(&modules, &bodies[loaf_modules..]);
    let mut witnessed = cacheable_bodies
        .iter()
        .chain(analysis.namespace_lowering_inputs())
        .map(|(identity, _)| identity)
        .collect::<std::collections::HashSet<_>>();
    let mut additional = Vec::new();
    for body in bodies[loaf_modules..].iter().flat_map(|module| &module.bodies) {
        if witnessed.insert(&body.direct_call_id) {
            let mut unwitnessed = body.clone();
            let witness = incan_semantics_core::semantic_digest::body_semantic_digest(&mut unwitnessed)
                .map_err(|error| format!("cannot witness namespace Body IR for lowering: {error}"))?;
            additional.push((body.direct_call_id.clone(), format!("namespace-body:{witness}")));
        }
    }
    cacheable_bodies.extend(additional);
    let settled = settle_lowering_witnesses(&mut cacheable_bodies, reads_by_module.values().flatten())?;
    let carried = cacheable_bodies.len();
    cacheable_bodies.extend(analysis.namespace_lowering_inputs().cloned());
    tracing::trace!(target: "incan::invocation", fresh_settled = settled, namespace_witnesses = cacheable_bodies.len() - carried,
        "prepared lowering inputs");
    let cacheable_bodies = canonical_lowering_inputs(cacheable_bodies)?;
    graph_body_output.attach_body_ir(
        &modules,
        &mut bodies[loaf_modules..],
        &cacheable_bodies,
        &reads_by_module,
        settled_inputs.as_ref(),
    )?;
    graph_body_output.publish(graph_body_records)?;
    let graph = if library && collect_codegraph {
        let records = incan_driver::inspect::codegraph::collect_checked_unit_codegraph(
            unit,
            &session,
            &modules,
            &analysis,
            &mut bodies[loaf_modules..],
        );
        session.loaves().validate_demands().map_err(|error| error.to_string())?;
        Some(incan_codegraph::artifact::encode_graph(&records, &unit.project_root).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let retained_namespaces = if resident && unit.kind == UnitKind::Program && !inspection {
        retained_namespace_bodies(
            &modules,
            &bodies[loaf_modules..],
            &restored,
            &rebound,
            graph_body_records,
            &unit.project_root,
            entry - loaf_modules,
        )
    } else {
        Vec::new()
    };
    sources.extend(modules.into_iter().map(|module| module.source));
    let program_entrypoints = program_entrypoints(unit, &bodies, &files)?;
    let entry_module = bodies.get_mut(entry).ok_or("selected Body IR entry is missing")?;
    entry_module.program_entrypoints = program_entrypoints;
    entry_module.string_conversion_contract = string_conversion_contract;
    entry_module.runtime_callable_bindings = runtime_callable_bindings;
    entry_module.runtime_binding_scopes = runtime_binding_scopes;
    let program = complete_dependency_context(
        CheckedProgram {
            cacheable_bodies,
            retained_namespaces,
            dependencies: RuntimeDependencyContext {
                loaves: std::sync::Arc::clone(session.loaves()),
                requested: requested.clone(),
                origins: requested
                    .iter()
                    .map(|identity| (identity.clone(), RootOrigin::Source))
                    .collect(),
                checked_local: checked_local.clone(),
                package_versions,
                package: package_identity.to_owned(),
                layout_modules: Vec::new(),
                sources: sources[loaf_modules..].to_vec(),
                files: files[loaf_modules..].to_vec(),
                entry: entry - loaf_modules,
            },
            modules: bodies,
            sources,
            files,
            entry,
            own: loaf_modules,
            dependency_modules: loaf_modules,
            reached_loaves,
            manifest,
            graph,
            tests,
        },
        session.loaves(),
        requested,
        package_identity,
        &checked_local,
    )?;
    session.loaves().validate_demands().map_err(|error| error.to_string())?;
    Ok(CheckedAttempt::Complete(program))
}

/// Resolve explicit program sources to checked main identities before joining module contexts. Ordinary imported
/// functions named main remain ordinary calls; only these selected identities acquire program entry semantics.
pub(super) fn program_entrypoints(
    unit: &UnitDescription,
    bodies: &[BodyIrModule],
    files: &[String],
) -> Result<Vec<CanonicalSymbolId>, String> {
    if !unit.program_entries.is_empty() && unit.kind != UnitKind::Program {
        return Err("program entries require a program unit".into());
    }
    let mut entries = Vec::new();
    for source in &unit.program_entries {
        let selected = unit.project_root.join(source);
        let path = selected
            .canonicalize()
            .map_err(|error| format!("cannot resolve program entry {}: {error}", selected.display()))?;
        let index = files
            .iter()
            .position(|file| Path::new(file) == path)
            .ok_or_else(|| format!("program entry {} is not a collected module", source.display()))?;
        let body = bodies[index]
            .bodies
            .iter()
            .find(|body| {
                body.name == "main"
                    && body
                        .canonical
                        .as_ref()
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function)
            })
            .ok_or_else(|| format!("program entry {} has no main", source.display()))?;
        let identity = body
            .canonical
            .clone()
            .ok_or("program entry has no canonical identity")?;
        if entries.contains(&identity) {
            return Err(format!("duplicate program entry {}", source.display()));
        }
        entries.push(identity);
    }
    Ok(entries)
}

/// Retain the producer's checked public identities for returning dependency edges. A foreign artifact gains no
/// permission to reach a private source declaration merely because this compilation happens to own it.
pub(super) fn checked_local_identities(manifest: Option<&LibraryManifest>) -> BTreeSet<CanonicalSymbolId> {
    manifest
        .into_iter()
        .flat_map(|manifest| {
            incan_frontend::library_manifest::public_identities::public_executable_identities(manifest)
                .into_iter()
                .filter(move |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == &manifest.name))
        })
        .collect()
}

/// A body borrowed for one native compilation, plus the exact source scopes it held on admission.
pub(super) struct RetainedNamespaceBody {
    source: String,
    offset: usize,
    scopes: Vec<incan_semantics_core::body_ir::SharedIdentities>,
    entry_union_count: Option<usize>,
}

/// A restored module can return its exact compiled Body IR once planning drops the invocation's native context.
/// Exclude modules with rewritten call locators or fields the context takes ownership of.
fn retained_namespace_bodies(
    modules: &[ParsedModule],
    authored: &[BodyIrModule],
    restored: &BTreeSet<String>,
    rebound: &BTreeSet<PathBuf>,
    records: &incan_frontend::checker_records::GraphBodyRecords,
    project: &Path,
    entry: usize,
) -> Vec<RetainedNamespaceBody> {
    modules
        .iter()
        .zip(authored)
        .enumerate()
        .filter_map(|(offset, (source, module))| {
            let relative = source.file_path.strip_prefix(project).ok()?.to_str()?;
            if !records.retains_namespace(relative)
                || (restored.contains(relative) && rebound.contains(&source.file_path))
                || !module.loaf_callables.is_empty()
                || module
                    .nominal_declarations
                    .iter()
                    .any(|layout| matches!(layout.canonical.origin, SymbolOrigin::RustCrate(_)))
                || module
                    .enum_declarations
                    .iter()
                    .any(|layout| matches!(layout.canonical.origin, SymbolOrigin::RustCrate(_)))
            {
                return None;
            }
            // Namespace publication precedes entry preparation. Keep the authored union prefix and admit an entry
            // only when its other invocation-owned fields are empty.
            let entry_union_count = if offset == entry {
                if !module.program_entrypoints.is_empty()
                    || !module.runtime_callable_bindings.is_empty()
                    || !module.runtime_binding_scopes.is_empty()
                    || !matches!(
                        module.string_conversion_contract,
                        incan_semantics_core::body_ir::StringConversionContract::Unselected
                    )
                {
                    return None;
                }
                Some(module.native_union_declarations.len())
            } else {
                None
            };
            // Runtime preparation may extend a scope with target-specific names. Keep only the original shared handles.
            let scopes = module
                .bodies
                .iter()
                .map(|body| std::sync::Arc::clone(&body.named_type_identities))
                .collect();
            Some(RetainedNamespaceBody {
                source: relative.to_owned(),
                offset,
                scopes,
                entry_union_count,
            })
        })
        .collect()
}

/// Move protected authored bodies back to their exact verified records. The native plan now owns its own products.
pub(super) fn return_retained_namespaces(
    program: &mut CheckedProgram,
    records: &incan_frontend::checker_records::GraphBodyRecords,
) {
    for retained in program.retained_namespaces.drain(..) {
        let Some(module) = program.modules.get_mut(program.own + retained.offset) else {
            continue;
        };
        if module.bodies.len() != retained.scopes.len() {
            continue;
        }
        if let Some(original) = retained.entry_union_count {
            if module.native_union_declarations.len() < original {
                continue;
            }
            module.native_union_declarations.truncate(original);
            module.program_entrypoints.clear();
            module.runtime_callable_bindings.clear();
            module.runtime_binding_scopes.clear();
            module.string_conversion_contract = incan_semantics_core::body_ir::StringConversionContract::Unselected;
            module.loaf_callables.clear();
        }
        for (body, scope) in module.bodies.iter_mut().zip(retained.scopes) {
            body.named_type_identities = scope;
        }
        records.retain_namespace(&retained.source, module);
    }
}

include!("checked_program/dependency_context.rs");

/// Replace standalone package bodies with linked contracts. Body IR keeps governed templates, including defaults
/// with an open Self receiver, available for checked adopter specialization. Move existing contracts into the
/// returned inventory so joining declaration modules cannot publish the same compiled callable twice.
pub(super) fn retain_linked_loaf_declarations(modules: &mut [BodyIrModule], linked: usize) -> Vec<LoafCallable> {
    let mut loaf_callables = modules
        .iter_mut()
        .flat_map(|module| std::mem::take(&mut module.loaf_callables))
        .collect::<Vec<_>>();
    for module in modules.iter_mut().take(linked) {
        for declaration in &mut module.nominal_declarations {
            if declaration.native_item.is_empty()
                && let Some(item) = linked_type_item(&declaration.canonical, &declaration.type_parameters)
            {
                declaration.native_item = item;
            }
        }
        for declaration in &mut module.enum_declarations {
            if let Some(item) = linked_type_item(&declaration.canonical, &declaration.type_parameters) {
                declaration.native_item = item;
            }
        }
        let governed = module
            .bodies
            .iter()
            .filter(|body| module.body_requires_instantiation(body))
            .filter_map(|body| body.canonical.clone())
            .collect::<std::collections::BTreeSet<_>>();
        module.bodies.retain(|body| {
            if body
                .canonical
                .as_ref()
                .is_some_and(|identity| governed.contains(identity))
            {
                return true;
            }
            match incan_semantics_core::body_ir::LoafCallable::from_body(body) {
                Some(callable) => {
                    loaf_callables.push(callable);
                    false
                }
                None => true,
            }
        });
    }
    loaf_callables
}

/// Move metadata-selected native layouts to their physical declaration scopes, deduplicating identical selections
/// across source modules. Conflicting layouts cannot authorize construction or projection.
pub(super) fn take_rust_layouts(modules: &mut [BodyIrModule]) -> Result<Vec<NominalDeclaration>, String> {
    let mut layouts = std::collections::BTreeMap::<String, NominalDeclaration>::new();
    for module in modules {
        for declaration in module.nominal_declarations.extract_if(.., |declaration| {
            matches!(declaration.canonical.origin, SymbolOrigin::RustCrate(_))
        }) {
            if let Some(previous) = layouts.get(&declaration.native_item) {
                if previous != &declaration {
                    return Err(format!(
                        "conflicting native field layouts for {}",
                        declaration.native_item
                    ));
                }
            } else {
                layouts.insert(declaration.native_item.clone(), declaration);
            }
        }
    }
    Ok(layouts.into_values().collect())
}

/// Select a native carrier or provider-owned Loaf layout. Generic models, classes and enums publish one parameterized
/// ADT; generic newtypes keep their governed source storage until their native templates are supported.
pub(super) fn linked_type_item(identity: &CanonicalSymbolId, parameters: &[String]) -> Option<String> {
    incan_semantics_core::body_ir::native_nominal_item(identity).or_else(|| {
        (parameters.is_empty()
            || matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Enum
            ))
        .then(|| incan_semantics_core::body_ir::loaf_item_path(identity))
        .flatten()
    })
}

/// Read each selected Loaf's published native union table, preserving its semantic carrier and physical variant
/// order. The index already selects exact artifacts; the adapter subsequently verifies each layout against metadata.
pub(super) fn native_union_layouts(
    loaves: &incan_frontend::loaf_index::LoafIndex,
) -> Vec<incan_semantics_core::body_ir::NativeUnionDeclaration> {
    use incan_frontend::library_manifest::resolved_type_from_manifest_type_ref;
    use incan_frontend::typechecker::semantic_type_from_resolved;
    loaves
        .native_union_entries()
        .flat_map(|entry| {
            entry
                .artifact
                .manifest
                .contract_metadata
                .native_unions
                .iter()
                .filter_map(|native| {
                    let mut callables = native.closure_members.iter();
                    let members = native
                        .members
                        .iter()
                        .map(|member| {
                            if matches!(member, incan_frontend::library_manifest::TypeRef::Function { .. }) {
                                callables.next().cloned()
                            } else {
                                Some(semantic_type_from_resolved(&resolved_type_from_manifest_type_ref(
                                    member,
                                )))
                            }
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some(incan_semantics_core::body_ir::NativeUnionDeclaration {
                        ty: native.semantic_type.clone()?,
                        rust_item: format!(
                            "{}::{}",
                            incan_semantics_core::body_ir::loaf_crate_name(&entry.artifact.manifest.name),
                            native.rust_name
                        ),
                        members,
                        closure_members: native.closure_members.clone(),
                        closure_items: native
                            .closure_items
                            .iter()
                            .map(|item| {
                                format!(
                                    "{}::{item}",
                                    incan_semantics_core::body_ir::loaf_crate_name(&entry.artifact.manifest.name),
                                )
                            })
                            .collect(),
                    })
                })
        })
        .collect()
}

/// Declare each Rust item the program holds in a declaration-only module under the item's native module: the
/// lowering lays a value of it out as the item itself.
pub(super) fn rust_value_modules(
    held: Vec<NominalDeclaration>,
    enums: Vec<incan_semantics_core::body_ir::EnumDeclaration>,
) -> Vec<BodyIrModule> {
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    for declaration in held {
        let Some(owner) = incan_semantics_core::body_ir::native_nominal_module(&declaration.canonical) else {
            continue;
        };
        owners
            .entry(owner.clone())
            .or_insert_with(|| BodyIrModule::declarations_only(owner))
            .nominal_declarations
            .push(declaration);
    }
    for declaration in enums {
        let Some(owner) = incan_semantics_core::body_ir::native_nominal_module(&declaration.canonical) else {
            continue;
        };
        owners
            .entry(owner.clone())
            .or_insert_with(|| BodyIrModule::declarations_only(owner))
            .enum_declarations
            .push(declaration);
    }
    owners.into_values().collect()
}

/// Hoist checked native enum layouts once per foreign owner, refusing conflicting templates from separate modules.
pub(super) fn take_rust_enums(
    modules: &mut [BodyIrModule],
) -> Result<Vec<incan_semantics_core::body_ir::EnumDeclaration>, String> {
    let mut layouts = std::collections::BTreeMap::new();
    for module in modules {
        for declaration in module.enum_declarations.extract_if(.., |declaration| {
            matches!(declaration.canonical.origin, SymbolOrigin::RustCrate(_))
        }) {
            if let Some(previous) = layouts.get(&declaration.native_item) {
                if previous != &declaration {
                    return Err(format!(
                        "conflicting native enum layouts for {}",
                        declaration.native_item
                    ));
                }
            } else {
                layouts.insert(declaration.native_item.clone(), declaration);
            }
        }
    }
    Ok(layouts.into_values().collect())
}

#[cfg(test)]
mod shared_identity_tests {
    use super::*;
    use incan_semantics_core::HirSourceSpan;
    use incan_semantics_core::body_ir::SharedIdentities;
    use std::sync::Arc;

    /// One canonical model identity for a fixture map entry.
    fn identity(name: &str) -> CanonicalSymbolId {
        CanonicalSymbolId::module_declaration(
            vec!["fixture".to_owned()],
            name,
            SemanticSourceTargetKind::Model,
            HirSourceSpan::new(0, 1),
        )
    }

    /// Bodies that share one identity map keep sharing one extended map, and a map that already names every identity
    /// is kept as it is, so a repeated runtime resolution adds no private copy per body.
    #[test]
    fn shared_identity_maps_stay_shared_when_extended() -> Result<(), Box<dyn std::error::Error>> {
        let module_wide: SharedIdentities = Arc::new(BTreeMap::from([("Owned".to_owned(), identity("Owned"))]));
        let complete: SharedIdentities = Arc::new(BTreeMap::from([("Native".to_owned(), identity("Native"))]));
        let mut maps = vec![
            Arc::clone(&module_wide),
            Arc::clone(&module_wide),
            Arc::clone(&complete),
        ];
        let native = BTreeMap::from([("Native".to_owned(), identity("Native"))]);

        extend_shared_identities(maps.iter_mut(), &native);

        let [first, second, kept] = &maps[..] else {
            return Err("the fixture holds three maps".into());
        };
        assert!(
            Arc::ptr_eq(first, second),
            "bodies that shared one map received private copies"
        );
        assert!(first.contains_key("Owned") && first.contains_key("Native"));
        assert!(
            Arc::ptr_eq(kept, &complete),
            "a map that already named every identity was copied"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_lowering_inputs;
    use incan_semantics_core::CompilerNodeId;

    /// Callable and namespace products may repeat one witness, but may not disagree about an owner.
    #[test]
    fn lowering_witnesses_are_unique_by_owner() -> Result<(), Box<dyn std::error::Error>> {
        let value = CompilerNodeId::declaration_span("main", 0, 12);
        let other = CompilerNodeId::declaration_span("main", 14, 30);
        let inputs = vec![
            (value.clone(), "body:a".to_owned()),
            (other.clone(), "body:b".to_owned()),
            (value.clone(), "body:a".to_owned()),
        ];
        assert_eq!(
            canonical_lowering_inputs(inputs)?,
            vec![(value.clone(), "body:a".to_owned()), (other, "body:b".to_owned())]
        );
        assert_eq!(
            canonical_lowering_inputs(vec![(value.clone(), "body:a".to_owned()), (value, "body:b".to_owned())]),
            Err("conflicting lowering witnesses for Body IR owner main#decl.0..12".to_owned())
        );
        Ok(())
    }
}
