//! Checked program assembly: runtime declarations, checked modules, entrypoints, dependency contexts and retained
//! native layouts.

use super::*;

/// Recover nonlocal call roots from accepted Body IR when a reused checker record no longer carries the reference
/// edge that originally selected the body. The canonical target is producer evidence; source spelling is not.
fn retained_body_call_requirements(modules: &[BodyIrModule], package_identity: &str) -> BTreeSet<CanonicalSymbolId> {
    let mut requirements = BTreeSet::new();
    for body in modules.iter().flat_map(|module| &module.bodies) {
        collect_retained_body_calls(&body.block.stmts, package_identity, &mut requirements);
    }
    requirements
}

fn collect_retained_body_calls(
    statements: &[incan_semantics_core::body_ir::Statement],
    package_identity: &str,
    requirements: &mut BTreeSet<CanonicalSymbolId>,
) {
    use incan_semantics_core::body_ir::{CallableTarget, Callee, Rvalue, StatementKind};

    for statement in statements {
        match &statement.kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } => {
                if let Some(identity) = &target.canonical
                    && matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity)
                {
                    requirements.insert(identity.clone());
                }
            }
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    collect_retained_body_calls(&arm.guard_stmts, package_identity, requirements);
                    collect_retained_body_calls(&arm.body_stmts, package_identity, requirements);
                }
            }
            StatementKind::Assign {
                rvalue: Rvalue::Closure { body, .. },
                ..
            } => collect_retained_body_calls(&body.stmts, package_identity, requirements),
            StatementKind::Assign {
                rvalue: Rvalue::Generator { body, .. },
                ..
            } => collect_retained_body_calls(&body.stmts, package_identity, requirements),
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_retained_body_calls(&then_block.stmts, package_identity, requirements);
                if let Some(block) = else_block {
                    collect_retained_body_calls(&block.stmts, package_identity, requirements);
                }
            }
            StatementKind::Loop { body } | StatementKind::UnsafeRegion { body, .. } => {
                collect_retained_body_calls(&body.stmts, package_identity, requirements);
            }
            StatementKind::Race { arms, .. } => {
                for arm in arms {
                    collect_retained_body_calls(&arm.body.stmts, package_identity, requirements);
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
        .collect::<std::collections::BTreeMap<_, _>>();
    for module in modules {
        for body in &mut module.bodies {
            for (name, identity) in &identities {
                std::sync::Arc::make_mut(&mut body.named_type_identities)
                    .entry(name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
    }
}

/// Retain canonical declarations for calls introduced by native lowering.
///
/// A bootstrap Rust runtime has no Loaf entry and contributes no governed bodies. A native runtime enters through
/// Oven's ordinary artifact index; its checked public identities select the same `.incnbody` records as source imports.
/// A lowering-provider producer uses its own checked export map, so specializing its source needs no external artifact.
pub(super) fn runtime_declarations(
    session: &CompilationSession,
    local: Option<&LibraryManifest>,
) -> Result<Vec<incan_semantics_core::body_ir::RuntimeCallableBinding>, String> {
    let mut bindings = Vec::new();
    for package in ["incan_native_runtime", "incan_stdlib_primitives"] {
        let projected;
        let manifest = if let Some(local) = local.filter(|manifest| manifest.name == package) {
            local
        } else {
            projected = session
                .loaves()
                .execution_manifest(package)
                .map_err(|error| error.to_string())?;
            let Some(projected) = projected.as_deref() else {
                continue;
            };
            projected
        };
        bindings.extend(
            manifest
                .contract_metadata
                .identity_graph
                .exports
                .iter()
                .filter(|export| export.public_path.len() == 2)
                .filter_map(|export| Some(incan_semantics_core::body_ir::RuntimeCallableBinding {
                    path: export.public_path.join("::"),
                    canonical: export.canonical.as_ref()?.hydrate()?,
                }))
                .filter(|binding| {
                    binding.canonical.kind == SemanticSourceTargetKind::Function
                        && matches!(&binding.canonical.origin, SymbolOrigin::Package { library, .. } if library == &manifest.name)
                }),
        );
    }
    Ok(bindings)
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
/// A refusal by source diagnostics or by declaration validation is certified through `receipt`.
pub(super) fn checked_modules(
    unit: &UnitDescription,
    rust: &RustFactsSetup,
    resident: bool,
    receipt: &RefusalReceipt,
    graph_body_records: incan_frontend::checker_records::GraphBodyRecords,
    graph_body_output: Option<PathBuf>,
    collect_codegraph: bool,
) -> Result<CheckedProgram, String> {
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
    session.set_graph_body_records(graph_body_records);
    let library = unit.kind == UnitKind::Library;
    let modules = if library {
        collect_library_modules_detailed_with_session(path.clone(), &session)
    } else {
        collect_modules_detailed_with_session(path.clone(), &session)
    }
    .map_err(|failure| receipt.diagnostic(failure))?;
    session
        .resolve_rust_facts(&modules)
        .map_err(|error| error.to_string())?;
    let tests = crate::test_unit::collect(unit, &session)?;
    let package_identity = unit.package.as_str();
    let mut callable_products = callable_products::CallableProducts::prepare(unit, &modules, &mut session);
    let mut graph_body_output =
        graph_body_output::GraphBodyOutput::prepare(graph_body_output, unit.project_root.clone())?;
    let mut publication = library.then(|| PublicationFacts::new(package_identity));
    let module_key_index = incan_driver::modules::module_key_index(&modules);
    let namespace_dependencies = modules
        .iter()
        .enumerate()
        .map(|(index, module)| {
            let mut dependencies = incan_driver::modules::imported_module_dep_indexes_for(
                &modules,
                index,
                &module_key_index,
                session.own_standard_package(),
            )
            .into_iter()
            .filter_map(|dependency| {
                modules[dependency]
                    .file_path
                    .strip_prefix(&unit.project_root)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
            dependencies.sort();
            dependencies.dedup();
            (module.file_path.clone(), dependencies)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let analysis = {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::analyze_modules", modules = modules.len()).entered();
        session
            .analyze_unit(&modules, library.then_some(package_identity), &mut |module, checker| {
                callable_products.observe(module, checker);
                graph_body_output.observe(
                    module,
                    checker,
                    namespace_dependencies
                        .get(&module.file_path)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                );
                if let Some(publication) = publication.as_mut() {
                    publication.visit(module, checker, module.file_path == path);
                }
            })
            .map_err(|failure| receipt.diagnostic(failure))?
    };
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
    let runtime_callable_bindings = runtime_declarations(&session, manifest.as_ref())?;
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
    lower_authored_modules(&modules, &analysis, &session, &mut bodies, &mut files)?;
    let retained_requirements = retained_body_call_requirements(&bodies[loaf_modules..], package_identity);
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
    let mut cacheable_bodies = callable_products.publish(&modules, &bodies[loaf_modules..]);
    cacheable_bodies.extend(analysis.namespace_lowering_inputs().iter().cloned());
    for body in bodies[loaf_modules..].iter().flat_map(|module| &module.bodies) {
        if cacheable_bodies
            .iter()
            .any(|(identity, _)| identity == &body.direct_call_id)
        {
            continue;
        }
        let mut witnessed = body.clone();
        let witness = incan_semantics_core::semantic_digest::body_semantic_digest(&mut witnessed)
            .map_err(|error| format!("cannot witness namespace Body IR for lowering: {error}"))?;
        cacheable_bodies.push((body.direct_call_id.clone(), format!("namespace-body:{witness}")));
    }
    graph_body_output.attach_body_ir(&modules, &bodies[loaf_modules..], &cacheable_bodies)?;
    graph_body_output.publish()?;
    let graph = if library && collect_codegraph {
        let records = incan_driver::inspect::codegraph::collect_checked_unit_codegraph(
            unit,
            &session,
            &modules,
            &analysis,
            &mut bodies[loaf_modules..],
        );
        Some(incan_codegraph::artifact::encode_graph(&records, &unit.project_root).map_err(|error| error.to_string())?)
    } else {
        None
    };
    sources.extend(modules.into_iter().map(|module| module.source));
    let program_entrypoints = program_entrypoints(unit, &bodies, &files)?;
    let entry_module = bodies.get_mut(entry).ok_or("selected Body IR entry is missing")?;
    entry_module.program_entrypoints = program_entrypoints;
    entry_module.string_conversion_contract = string_conversion_contract;
    entry_module.runtime_callable_bindings = runtime_callable_bindings;
    let program = complete_dependency_context(
        CheckedProgram {
            cacheable_bodies,
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
                layout_modules_installed: false,
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
    Ok(program)
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

/// Complete selected defaults and native layouts before the first planning attempt.
pub(super) fn complete_dependency_context(
    mut program: CheckedProgram,
    loaves: &LoafIndex,
    requested: BTreeSet<CanonicalSymbolId>,
    package_identity: &str,
    checked_local: &BTreeSet<CanonicalSymbolId>,
) -> Result<CheckedProgram, String> {
    complete_dependency_context_in_place(&mut program, loaves, requested, package_identity, checked_local)?;
    Ok(program)
}

/// Refresh selected defaults, linked callable contracts and held native layouts without publishing an attempt.
pub(super) fn complete_dependency_context_in_place(
    program: &mut CheckedProgram,
    loaves: &LoafIndex,
    mut requested: BTreeSet<CanonicalSymbolId>,
    package_identity: &str,
    checked_local: &BTreeSet<CanonicalSymbolId>,
) -> Result<(), String> {
    let comparisons = incan_frontend::body_ir::required_comparison_defaults(&program.modules[program.own..])
        .into_iter()
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity),
        )
        .map(|identity| (identity, RootOrigin::ComparisonDefault));
    let adapters = incan_frontend::body_ir::required_primitive_adapters(loaves, &requested)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|identity| (identity, RootOrigin::PrimitiveAdapter));
    let defaults = comparisons.chain(adapters).collect::<BTreeMap<_, _>>();
    if !defaults.keys().all(|identity| requested.contains(identity)) {
        for (identity, origin) in defaults {
            if requested.insert(identity.clone()) {
                program.dependencies.origins.insert(identity, origin);
            }
        }
        let resolved = load_loaf_modules(loaves, &requested, checked_local)?;
        install_dependency_modules(program, resolved);
    }
    program.dependencies.requested = requested;
    let loaf_callables = retain_linked_loaf_declarations(&mut program.modules, program.own);
    program
        .modules
        .get_mut(program.entry)
        .ok_or("selected Body IR entry is missing")?
        .loaf_callables = loaf_callables;
    // A Rust layout's storage stays rustc's: find the Rust items bodies hold only once the layouts are taken, so a
    // field no body reads does not pull its type's private items into the unit. A read field's value is a checked
    // body local or external signature type, which still counts.
    let layouts = take_rust_layouts(&mut program.modules)?;
    let opaque = incan_semantics_core::body_ir::held_rust_items(&program.modules);
    let native_enums = take_rust_enums(&mut program.modules)?;
    let owners: std::collections::HashSet<_> = layouts
        .iter()
        .map(|declaration| declaration.canonical.clone())
        .chain(native_enums.iter().map(|declaration| declaration.canonical.clone()))
        .collect();
    let held = rust_value_modules(
        layouts
            .into_iter()
            .chain(
                opaque
                    .into_iter()
                    .filter(|declaration| !owners.contains(&declaration.canonical)),
            )
            .collect(),
        native_enums,
    );
    retain_native_type_identities(&mut program.modules, &held);
    install_rust_value_modules(program, held);
    // A program whose checked types hold no union selects no union layout; its producers' manifests stay unread.
    let layouts = if incan_frontend::body_ir::reaches_union(&program.modules) {
        native_union_layouts(loaves)
    } else {
        Vec::new()
    };
    let entry_module = program
        .modules
        .get_mut(program.entry)
        .ok_or("selected Body IR entry is missing")?;
    entry_module.native_union_declarations.extend(layouts);
    Ok(())
}

/// Resolve a nonempty set selected by lowering, then reconstruct layout context from the active authored suffix.
/// The canonical root set must grow on every retry; a repeated missing body is an invalid producer contract.
pub(super) fn satisfy_runtime_requirements(
    program: &mut CheckedProgram,
    requirements: Vec<RuntimeRequirement>,
) -> Result<Vec<usize>, String> {
    let previous = runtime_module_generations(&program.modules)?;
    let mut next = program.dependencies.requested.clone();
    for requirement in &requirements {
        if !next.insert(requirement.canonical.clone()) {
            return Err(format!(
                "runtime declaration `{}` remained unavailable after resolution at {}:{}:{}",
                requirement.path, requirement.span.file, requirement.span.line, requirement.span.column,
            ));
        }
    }
    if requirements.is_empty() {
        return Err("runtime planning requested an empty dependency set".to_owned());
    }
    let resolved = load_loaf_modules(&program.dependencies.loaves, &next, &program.dependencies.checked_local)
        .map_err(|error| {
            let sites = requirements
                .iter()
                .map(|requirement| {
                    format!(
                        "{} at {}:{}:{}",
                        requirement.path, requirement.span.file, requirement.span.line, requirement.span.column
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{error}; selected runtime operations: {sites}")
        })?;
    install_dependency_modules(program, resolved);
    let context = &mut program.dependencies;
    for requirement in &requirements {
        context.origins.insert(
            requirement.canonical.clone(),
            RootOrigin::Runtime {
                path: requirement.path.clone(),
                file: requirement.span.file.to_string(),
                line: requirement.span.line,
                column: requirement.span.column,
            },
        );
    }
    context.requested = next;
    prepare_dependency_context(program)?;
    changed_runtime_module_indices(program, &previous)
}

/// Record the compact monotonic tables that dependency reconstruction can extend between lowering attempts.
fn runtime_module_generations(modules: &[BodyIrModule]) -> Result<BTreeMap<String, [usize; 11]>, String> {
    let mut generations = BTreeMap::new();
    for module in modules {
        let owner = module.module_id.path().to_owned();
        let generation = [
            module.bodies.len(),
            module.nominal_declarations.len(),
            module.enum_declarations.len(),
            module.fieldless_enum_declarations.len(),
            module.value_enum_declarations.len(),
            module.trait_declarations.len(),
            module.trait_implementations.len(),
            module.static_declarations.len(),
            module.decorated_bindings.len(),
            module.loaf_callables.len(),
            module.native_union_declarations.len(),
        ];
        if generations.insert(owner.clone(), generation).is_some() {
            return Err(format!("duplicate Body IR module owner `{owner}`"));
        }
    }
    Ok(generations)
}

/// Select only modules whose monotonic checked tables grew, plus the entry whose compact linkage facts are refreshed.
fn changed_runtime_module_indices(
    program: &CheckedProgram,
    previous: &BTreeMap<String, [usize; 11]>,
) -> Result<Vec<usize>, String> {
    let current = runtime_module_generations(&program.modules)?;
    let mut selected = Vec::new();
    for (index, module) in program.modules.iter().enumerate() {
        let owner = module.module_id.path();
        if previous.get(owner) != current.get(owner) {
            selected.push(index);
        }
    }
    if !selected.contains(&program.entry) {
        selected.push(program.entry);
        selected.sort_unstable();
    }
    Ok(selected)
}

/// Retain layout-authoritative runtime declarations without claiming that the unit selected their operations.
///
/// Only nominal and enum declarations survive. They remain outside `own`, have no executable bodies or storage and
/// accompany every later dependency reconstruction, while reporting and linking continue to use the exact reached
/// closure.
pub(super) fn prepare_runtime_layouts(
    program: &mut CheckedProgram,
    requirements: Vec<RuntimeRequirement>,
) -> Result<(), String> {
    if requirements.is_empty() {
        return Err("runtime layout preparation requested an empty declaration set".to_owned());
    }
    let requested = requirements
        .into_iter()
        .map(|requirement| requirement.canonical)
        .collect::<BTreeSet<_>>();
    let resolved = load_loaf_modules(
        &program.dependencies.loaves,
        &requested,
        &program.dependencies.checked_local,
    )?;
    program.dependencies.package_versions.extend(resolved.package_versions);
    let mut layouts = Vec::new();
    for mut module in resolved.modules {
        let mut layout = BodyIrModule::declarations_only(module.module_id.path().to_owned());
        layout.nominal_declarations = std::mem::take(&mut module.nominal_declarations);
        layout.enum_declarations = std::mem::take(&mut module.enum_declarations);
        if !layout.nominal_declarations.is_empty() || !layout.enum_declarations.is_empty() {
            layouts.push(layout);
        }
    }
    program.dependencies.layout_modules = layouts;
    let resolved = load_loaf_modules(
        &program.dependencies.loaves,
        &program.dependencies.requested,
        &program.dependencies.checked_local,
    )?;
    install_dependency_modules(program, resolved);
    prepare_dependency_context(program)
}

/// Move prepared and authored modules between contexts while replacing only reached dependencies.
///
/// Every preparation splits the authored suffix from the active program and retains its prepared layout prefix while
/// discarding only the prior reached dependency prefix. The retry therefore needs no second authored Body IR copy.
fn install_dependency_modules(
    program: &mut CheckedProgram,
    resolved: incan_frontend::executable_resolution::ResolvedExecutableModules,
) {
    program.dependencies.package_versions.extend(resolved.package_versions);
    let mut previous = std::mem::take(&mut program.modules);
    let mut authored = previous.split_off(program.own);
    let mut prepared = previous.split_off(program.dependency_modules);
    if !program.dependencies.layout_modules_installed {
        prepared.extend(program.dependencies.layout_modules.iter().cloned());
        program.dependencies.layout_modules_installed = true;
    }
    program.files.clear();
    program.sources.clear();
    let mut modules = resolved.modules;
    let dependency_modules = modules.len();
    for module in prepared {
        let owner = module.module_id.path().to_owned();
        match modules.iter_mut().find(|candidate| candidate.module_id.path() == owner) {
            Some(existing) => merge_prepared_layouts(existing, module),
            None => modules.push(module),
        }
    }
    let own = modules.len();
    let mut files = modules
        .iter()
        .map(|module| module.module_id.path().to_owned())
        .collect::<Vec<_>>();
    files.extend(program.dependencies.files.iter().cloned());
    let mut sources = vec![String::new(); own];
    sources.extend(program.dependencies.sources.iter().cloned());
    modules.append(&mut authored);
    program.modules = modules;
    program.files = files;
    program.sources = sources;
    program.entry = own + program.dependencies.entry;
    program.own = own;
    program.dependency_modules = dependency_modules;
    program.reached_loaves = resolved.entries;
}

/// Merge retained declaration-only layout authority into a newly resolved executable module of the same owner.
///
/// Runtime layout preparation may discover a nominal or enum before dependency resolution selects an executable
/// declaration from that module. The executable module becomes the single provenance authority while its prepared,
/// layout-authoritative records replace the same canonical declarations and fill declarations absent from the
/// executable selection.
fn merge_prepared_layouts(existing: &mut BodyIrModule, prepared: BodyIrModule) {
    for declaration in prepared.nominal_declarations {
        let position = existing
            .nominal_declarations
            .iter()
            .position(|candidate| candidate.canonical == declaration.canonical);
        match position {
            Some(position) => existing.nominal_declarations[position] = declaration,
            None => existing.nominal_declarations.push(declaration),
        }
    }
    for declaration in prepared.enum_declarations {
        let position = existing
            .enum_declarations
            .iter()
            .position(|candidate| candidate.canonical == declaration.canonical);
        match position {
            Some(position) => existing.enum_declarations[position] = declaration,
            None => existing.enum_declarations.push(declaration),
        }
    }
}

/// Refresh physical native layout modules across dependency retries without creating a second source authority.
///
/// Completing a dependency context takes Rust declarations out of every active module and groups them under their
/// physical native owner. On a retry, those owners already live in the retained prepared prefix: replace their
/// emptied modules in place and splice only owners that this program has not held before. Module, file and source
/// vectors remain aligned so lowering can require exactly one provenance entry per canonical module.
fn install_rust_value_modules(program: &mut CheckedProgram, held: Vec<BodyIrModule>) {
    let mut additions = Vec::new();
    for module in held {
        let owner = module.module_id.path().to_owned();
        let existing = program.modules[program.dependency_modules..program.own]
            .iter()
            .position(|candidate| candidate.module_id.path() == owner)
            .map(|position| program.dependency_modules + position);
        match existing {
            Some(position) => {
                program.modules[position] = module;
                program.files[position] = owner;
                program.sources[position].clear();
            }
            None => additions.push(module),
        }
    }
    let added = additions.len();
    program.files.splice(
        program.own..program.own,
        additions.iter().map(|module| module.module_id.path().to_owned()),
    );
    program
        .sources
        .splice(program.own..program.own, additions.iter().map(|_| String::new()));
    program.modules.splice(program.own..program.own, additions);
    program.entry += added;
    program.own += added;
}

/// Apply the existing context completion routine to a borrowed program without changing dependency ownership.
pub(super) fn prepare_dependency_context(program: &mut CheckedProgram) -> Result<(), String> {
    let loaves = std::sync::Arc::clone(&program.dependencies.loaves);
    let requested = program.dependencies.requested.clone();
    let checked_local = program.dependencies.checked_local.clone();
    let package = program.dependencies.package.clone();
    complete_dependency_context_in_place(program, &loaves, requested, &package, &checked_local)?;
    loaves.validate_demands().map_err(|error| error.to_string())
}

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
        for declaration in std::mem::take(&mut module.nominal_declarations) {
            if !matches!(
                declaration.canonical.origin,
                incan_semantics_core::SymbolOrigin::RustCrate(_)
            ) {
                module.nominal_declarations.push(declaration);
                continue;
            }
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
        for declaration in std::mem::take(&mut module.enum_declarations) {
            if !matches!(
                declaration.canonical.origin,
                incan_semantics_core::SymbolOrigin::RustCrate(_)
            ) {
                module.enum_declarations.push(declaration);
                continue;
            }
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
