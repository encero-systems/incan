// Settle runtime dependencies against preserved ordinary frames. Included by `lowering.rs`.

/// Resume preserved ordinary frames against the exact runtime declaration closure the frontend resolved.
pub fn resume_unit_plan(
    continuation: UnitContinuation,
    modules: &[BodyIrModule],
    inputs: &UnitInputs<'_>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    validate_plan_inputs(
        modules,
        inputs.sources,
        inputs.files,
        inputs.entry,
        &continuation.context,
    )?;
    let entry = checked_entry(modules, inputs.entry)?;
    let modules: Vec<_> = modules
        .iter()
        .zip(inputs.sources.iter().zip(inputs.files))
        .map(|(module, (source, file))| RuntimeModule { module, source, file })
        .collect();
    complete_runtime_plan(
        continuation,
        &modules,
        entry,
        inputs.kind,
        inputs.tests,
        inputs.package_versions,
        inputs.linked_static_exports,
        published,
    )
}

/// Borrow the exact checked delta from the same request. The shared completion validates introduced facts before
/// projection and resumes the ordinary frames already validated by this request.
pub fn complete_recorded_runtime_plan(
    continuation: UnitContinuation,
    inputs: &crate::RecordedRuntimeInputs<'_>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let mut modules = Vec::with_capacity(inputs.selected.len());
    let mut entry = None;
    for &index in inputs.selected {
        let module = inputs.modules.get(index).ok_or("recorded runtime module is missing")?;
        let source = inputs.sources.get(index).ok_or("recorded runtime source is missing")?;
        let file = inputs.files.get(index).ok_or("recorded runtime file is missing")?;
        if index == inputs.entry {
            entry = Some(modules.len());
        }
        modules.push(RuntimeModule { module, source, file });
    }
    complete_runtime_plan(
        continuation,
        &modules,
        entry.ok_or("recorded runtime selection omitted the entry")?,
        inputs.kind,
        inputs.tests,
        inputs.package_versions,
        inputs.linked_static_exports,
        published,
    )
}

/// Complete either runtime route in exactly the order of the preserved ordinary layouts and frames.
#[allow(clippy::too_many_arguments)]
fn complete_runtime_plan(
    mut continuation: UnitContinuation,
    modules: &[RuntimeModule<'_>],
    entry: usize,
    kind: &str,
    tests: &[String],
    package_versions: &BTreeMap<String, String>,
    linked_static_exports: &BTreeMap<CanonicalSymbolId, LinkedStaticAccessors>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let trace = traced("INCAN_TRACE_LOWERING");
    let source_files = runtime_source_files(modules)?;
    let previous_nominal_count = continuation.context.nominal_declarations.len();
    let previous_model_count = continuation.models.len();
    let previous_static_count = continuation.context.static_declarations.len();
    let (added_context, callables_changed) =
        extend_continuation_for_entry(&mut continuation, modules, entry, package_versions)?;
    let retained_union_count = refresh_continuation_layouts(
        &mut continuation,
        &source_files,
        package_versions,
        added_context || callables_changed,
    )?;
    let c = &mut continuation;
    let mut inventories = Inventories {
        models: std::mem::take(&mut c.models),
        enums: std::mem::take(&mut c.enums),
        functions: std::mem::take(&mut c.functions),
        externals: std::mem::take(&mut c.externals),
        instances: std::mem::take(&mut c.instances),
    };
    resume_plan_roots(
        &mut c.program,
        c.constructors.clone(),
        &c.context,
        &c.entry_file,
        &mut c.intrinsic_layouts,
        &c.nominal_instances,
        kind,
        &c.unions[retained_union_count..],
        previous_nominal_count,
        previous_model_count,
        &mut inventories,
    )?;
    let resolved_statics = lower_computed_statics(
        &c.context,
        &mut c.program,
        &c.unions,
        &c.sources,
        &c.files,
        &c.owners,
        &mut c.intrinsic_layouts,
        &mut inventories,
        previous_static_count,
    )?;
    c.emitted_frames = rebase_emitted_frames(&c.context, &c.emitted_frames);
    c.statics.extend(resolved_statics);
    let requirements = selected_runtime_requirements(&c.context, &inventories.externals, &inventories.functions)?;
    if !requirements.is_empty() {
        c.layout_inputs_grew = false;
        c.models = inventories.models;
        c.enums = inventories.enums;
        c.functions = inventories.functions;
        c.externals = inventories.externals;
        c.instances = inventories.instances;
        return Ok(UnitPlanning::NeedsRuntime(continuation, requirements));
    }
    finish_unit_plan(
        Finishing {
            template_constructors: continuation.template_constructors,
            constructors: continuation.constructors,
            source_context: continuation.source_context,
            template_references: continuation.template_references,
            context: continuation.context,
            program: continuation.program,
            owners: continuation.owners,
            sources: continuation.sources,
            files: continuation.files,
            entry_owner: continuation.entry_owner,
            entry_file: continuation.entry_file,
            intrinsics: continuation.intrinsic_layouts,
            closed_carrier_signatures: continuation.closed_carrier_signatures,
            inventories,
            emitted_frames: continuation.emitted_frames,
            statics: continuation.statics,
            unions: continuation.unions,
            retained_nominals: continuation.nominal_instances,
            products: continuation.instance_products,
        },
        kind,
        tests,
        linked_static_exports,
        published,
        trace,
    )
}

/// Index each runtime module's source file by its Body IR module path; one module has one source.
fn runtime_source_files(modules: &[RuntimeModule<'_>]) -> Result<BTreeMap<String, String>, String> {
    let mut source_files = BTreeMap::new();
    for row in modules {
        let path = row.module.module_id.path().to_owned();
        if source_files.insert(path.clone(), row.file.to_owned()).is_some() {
            return Err(format!("duplicate Body IR source provenance for module {path}"));
        }
    }
    Ok(source_files)
}

/// Refresh the entry's callables and plan any context modules the entry adds, extending the continuation's source
/// context, owners, retained sources and nominal instances. Returns whether context was added and whether the entry's
/// callables changed.
fn extend_continuation_for_entry(
    c: &mut UnitContinuation,
    modules: &[RuntimeModule<'_>],
    entry: usize,
    package_versions: &BTreeMap<String, String>,
) -> Result<(bool, bool), String> {
    let callables_changed = refresh_entry_context(Arc::make_mut(&mut c.context), modules[entry].module);
    let additions = continuation_inputs(&c.source_context, &c.context, &c.owners, modules, entry)?;
    let added_context = !additions.modules.is_empty();
    if added_context {
        let addition_source_files = source_file_index(&additions.modules, &additions.files)?;
        let added = plan_contexts_parallel(
            additions.modules.clone(),
            &additions.sources,
            &additions.files,
            additions.entry as i64,
            false,
            &addition_source_files,
            package_versions,
            &c.context,
            &c.template_constructors,
        )?;
        extend_continuation_context(
            Arc::make_mut(&mut c.source_context),
            Arc::make_mut(&mut c.context),
            Arc::make_mut(&mut c.template_constructors),
            &mut c.owners,
            &mut c.sources,
            &mut c.files,
            &mut c.template_references,
            &mut c.nominal_instances,
            &additions,
            &added.source_context,
            &added.context,
            &added.template_references,
            &added.nominal_instances,
        )?;
    }
    Ok((added_context, callables_changed))
}

/// Extend the constructor authority and index with the continued context, and rebuild the body index, unions and
/// intrinsic layouts when the context or its callables changed or a suspended instance pass grew the layout inputs.
/// Linkage-only batches change runtime scopes without changing the inputs these layouts consumed. Returns how many
/// unions were retained before this batch.
fn refresh_continuation_layouts(
    c: &mut UnitContinuation,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
    context_changed: bool,
) -> Result<usize, String> {
    extend_constructor_authority(
        Arc::make_mut(&mut c.template_constructors),
        source_files,
        package_versions,
    );
    extend_constructor_index(Arc::make_mut(&mut c.constructors), &c.context.nominal_declarations);
    extend_generic_constructor_arities(Arc::make_mut(&mut c.constructors), &c.template_constructors);
    extend_constructor_authority(Arc::make_mut(&mut c.constructors), source_files, package_versions);
    let retained_union_count = c.unions.len();
    if context_changed || c.layout_inputs_grew {
        extend_body_index(&mut c.program.body_indices, &c.context)?;
        extend_unions(&mut c.unions, &c.context);
        c.intrinsic_layouts = intrinsic_declarations(
            &c.constructors,
            &c.context,
            &c.program.body_indices,
            &c.entry_file,
            std::mem::take(&mut c.intrinsic_layouts),
            "continuation",
            Some(&mut c.closed_carrier_signatures),
        )?;
    }
    Ok(retained_union_count)
}
