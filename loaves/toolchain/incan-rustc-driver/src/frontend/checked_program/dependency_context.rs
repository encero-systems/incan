// Completing a checked program's dependency context: selected defaults and native layouts, runtime requirements bound
// to their generations, prepared layouts merged and dependency modules installed. Included by `checked_program.rs`.

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
    // body local or external signature type, which still counts. A constructed layout writes every field, so its field
    // types count too, even when only a dependency names them.
    let layouts = take_rust_layouts(&mut program.modules)?;
    let mut opaque = incan_semantics_core::body_ir::held_rust_items(&program.modules);
    let constructed = incan_semantics_core::body_ir::constructed_rust_field_items(&program.modules, &layouts, &opaque);
    opaque.extend(constructed);
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

/// Complete only a selected path whose canonical declaration does not yet exist in the planning context.
fn bind_runtime_requirement(
    program: &mut CheckedProgram,
    requirement: &RuntimeRequirement,
) -> Result<Option<CanonicalSymbolId>, String> {
    if let Some(identity) = &requirement.canonical {
        return Ok(Some(identity.clone()));
    }
    let (package, _) = requirement
        .path
        .split_once("::")
        .ok_or("runtime requirement does not name its package")?;
    let binding = program
        .dependencies
        .loaves
        .runtime_binding(package, &requirement.path)
        .map_err(|error| error.to_string())?;
    let entry = program
        .modules
        .get_mut(program.entry)
        .ok_or("selected Body IR entry is missing")?;
    let Some(binding) = binding else {
        let (_, native_paths) = entry
            .runtime_binding_scopes
            .iter_mut()
            .find(|(name, _)| name == package)
            .ok_or_else(|| format!("runtime request outside selected package: {}", requirement.path))?;
        if native_paths.contains(&requirement.path) {
            return Err(format!(
                "runtime request repeated after native linkage was established: {}",
                requirement.path
            ));
        }
        native_paths.push(requirement.path.clone());
        return Ok(None);
    };
    let identity = binding.canonical.clone();
    if !entry.runtime_callable_bindings.contains(&binding) {
        entry.runtime_callable_bindings.push(binding);
    }
    Ok(Some(identity))
}

/// Resolve a nonempty set selected by lowering, then reconstruct layout context from the active authored suffix.
/// The canonical root set must grow on every retry; a repeated missing body is an invalid producer contract.
pub(super) fn satisfy_runtime_requirements(
    program: &mut CheckedProgram,
    requirements: Vec<RuntimeRequirement>,
) -> Result<Vec<usize>, String> {
    let previous = runtime_module_generations(&program.modules)?;
    let mut next = program.dependencies.requested.clone();
    let mut resolved_requirements = Vec::new();
    for requirement in &requirements {
        let Some(identity) = bind_runtime_requirement(program, requirement)? else {
            continue;
        };
        if program.dependencies.requested.contains(&identity) && requirement.canonical.is_some() {
            return Err(format!(
                "runtime declaration `{}` remained unavailable after resolution at {}:{}:{}",
                requirement.path, requirement.span.file, requirement.span.line, requirement.span.column,
            ));
        }
        next.insert(identity.clone());
        resolved_requirements.push((requirement, identity));
    }
    if requirements.is_empty() {
        return Err("runtime planning requested an empty dependency set".to_owned());
    }
    if next == program.dependencies.requested {
        return changed_runtime_module_indices(program, &previous);
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
    for (requirement, identity) in resolved_requirements {
        context.origins.entry(identity).or_insert_with(|| RootOrigin::Runtime {
            path: requirement.path.clone(),
            file: requirement.span.file.to_string(),
            line: requirement.span.line,
            column: requirement.span.column,
        });
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
    // checked_modules already completed the reached dependency context. With no extra layout roots the same
    // authenticated fragments and prepared layout owners are still resident in this program.
    if requirements.is_empty() {
        return Ok(());
    }
    let mut requested = BTreeSet::new();
    for requirement in &requirements {
        requested.extend(bind_runtime_requirement(program, requirement)?);
    }
    if requested.is_empty() {
        return Ok(());
    }
    let mut layouts = Vec::new();
    let resolved = load_loaf_modules(
        &program.dependencies.loaves,
        &requested,
        &program.dependencies.checked_local,
    )?;
    program.dependencies.package_versions.extend(resolved.package_versions);
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
    // A comparison default can extend the reached set before layout preparation supplies its modules.
    // Consume only the layouts this pass actually received; earlier empty passes cannot suppress them.
    prepared.append(&mut program.dependencies.layout_modules);
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
