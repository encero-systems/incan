// The report commands: the unit's settled dependency closure with the Rust runtime operations its plan reaches, and
// the checked source interface. Included by `frontend.rs`.

/// Print the unit's settled dependency closure. Checking and planning run exactly as for compilation, so the report
/// names the closure a build of the same description links; nothing is generated or published.
pub fn dependency_report(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [unit_path, sysroot, options @ ..] = arguments else {
        return Err("usage: --dependencies UNIT SYSROOT [--library]".into());
    };
    crate::identity::verify(Path::new(sysroot))?;
    let mut unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    let dependencies = dependencies(&unit, options)?;
    if dependencies.library {
        unit.kind = UnitKind::Library;
    }
    let (kind, source, project_root) = (unit.kind, unit.source.clone(), unit.project_root.clone());
    let overflow_checks = unit.profile != "release";
    let rust = dependencies.rust_facts(&unit, Path::new(sysroot));
    let receipt = RefusalReceipt(None);
    let mut program = check_compilation_source(
        unit,
        rust,
        false,
        receipt.clone(),
        dependencies.graph_body_records,
        dependencies.graph_body_output,
        dependencies.collect_codegraph,
    )?;
    let plan = lower_compilation_plan(
        &mut program,
        kind,
        overflow_checks,
        &source,
        &receipt,
        &mut Vec::new(),
        None,
        Arc::default(),
        None,
    )?;
    let native_runtime = native_runtime_operations(&plan, &program.dependencies.origins);
    // The dependency inspection's roots come from checked source and final planning, independently of the
    // storage reads of the measured build. Preserve canonical identities (including overloads) for replay.
    if tracing::enabled!(target: "incan::invocation", tracing::Level::TRACE) {
        let mut roots = 0usize;
        for identity in program
            .dependencies
            .requested
            .difference(&program.dependencies.checked_local)
        {
            let SymbolOrigin::Package { library, .. } = &identity.origin else {
                continue;
            };
            let runtime = match program.dependencies.origins.get(identity) {
                Some(RootOrigin::Runtime { path, .. }) => path.as_str(),
                _ => "",
            };
            tracing::trace!(
                target: "incan::invocation", phase = "dependency_root", source = %source.display(), loaf = %library,
                identity = %incan_semantics_core::encode_incan_symbol_identity(identity), runtime,
            );
            roots += 1;
        }
        tracing::trace!(target: "incan::invocation", phase = "dependency_roots_complete", source = %source.display(), roots);
    }
    print!(
        "{}",
        crate::dependency_report::render(
            &program.dependencies.package,
            kind,
            &program.reached_loaves,
            &program.dependencies.origins,
            &native_runtime,
            &project_root,
        )
    );
    Ok(())
}

/// The runtime functions a plan calls natively, each with the earliest site that calls it. A runtime operation that
/// entered a runtime declaration is already a reached root; the Rust runtime publishes none, so its calls appear here.
fn native_runtime_operations(
    plan: &crate::plan::Plan,
    origins: &BTreeMap<CanonicalSymbolId, RootOrigin>,
) -> BTreeMap<String, RootOrigin> {
    let entered: BTreeSet<&str> = origins
        .values()
        .filter_map(|origin| match origin {
            RootOrigin::Runtime { path, .. } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    let mut sites: BTreeMap<&str, (String, i64, i64)> = BTreeMap::new();
    for external in &plan.externals {
        let path = external.path.as_str();
        if !path.starts_with("incan_native_runtime::") || entered.contains(path) {
            continue;
        }
        let site = (external.span.file.to_string(), external.span.line, external.span.column);
        match sites.get_mut(path) {
            Some(kept) if site < *kept => *kept = site,
            Some(_) => {}
            None => {
                sites.insert(path, site);
            }
        }
    }
    sites
        .into_iter()
        .map(|(path, (file, line, column))| {
            let origin = RootOrigin::Runtime {
                path: path.to_owned(),
                file,
                line,
                column,
            };
            (path.to_owned(), origin)
        })
        .collect()
}

/// The names of the packages the checked declaration closure reaches.
fn reached_names(program: &CheckedProgram) -> BTreeSet<String> {
    program.reached_loaves.keys().cloned().collect()
}

/// Publish checked declarations and governed bodies before a library's native artifact exists. This uses the same
/// source-to-Body-IR pass and publication as compilation, but provides no compiled callable or native layout. It
/// breaks source dependency cycles such as the runtime's standard trait bounds without importing a prior runtime.
/// Consumers that require the provider's compiled crate still refuse until ordinary compilation supplies it.
pub fn source_interface(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    crate::native_timing::observe_frontend(false);
    let [unit_path, sysroot, directory, options @ ..] = arguments else {
        return Err(
            "usage: --source-interface UNIT SYSROOT DIRECTORY [--toolchain-root DIR] [--bakery-root DIR]".into(),
        );
    };

    // ---- Checking inputs without an existing native publication ----
    crate::identity::verify(Path::new(sysroot))?;
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    if unit.kind != UnitKind::Library {
        return Err("a source interface requires a library unit".into());
    }
    let directory = Path::new(directory);
    let library = directory.join(format!("lib{}.rlib", native_crate_name(&unit.package)));
    if library.exists() {
        return Err("a source interface cannot replace a compiled library's publication".into());
    }
    let dependencies = dependencies(&unit, options)?;
    let rust = dependencies.rust_facts(&unit, Path::new(sysroot));
    let loaf_links = unit.loaves.iter().map(|loaf| native_crate_name(&loaf.name)).collect();
    let mut closure = LoafNativeClosure {
        externs: dependencies.externs,
        search: unit.own_search.clone(),
        macro_externs: Vec::new(),
    };

    // ---- Checked source and governed publication ----
    let program = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        checked_modules(
            &unit,
            &rust,
            false,
            &RefusalReceipt(None),
            dependencies.graph_body_records,
            None,
            true,
            dependencies.inspection,
        )
    })?;
    let manifest = program
        .manifest
        .ok_or("a checked source interface requires a library manifest")?;
    closure.macro_externs = macro_externs(&manifest, &closure.externs, &loaf_links);
    std::fs::create_dir_all(directory)?;
    write_loaf(
        directory,
        manifest,
        &program.modules[program.own..],
        &closure,
        &dependencies.anchors,
        program.graph.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}
