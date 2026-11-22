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
    continuation: UnitContinuation,
    modules: &[RuntimeModule<'_>],
    entry: usize,
    kind: &str,
    tests: &[String],
    package_versions: &BTreeMap<String, String>,
    linked_static_exports: &BTreeMap<CanonicalSymbolId, LinkedStaticAccessors>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let trace = traced("INCAN_TRACE_LOWERING");
    let mut source_files = BTreeMap::new();
    for row in modules {
        let path = row.module.module_id.path().to_owned();
        if source_files.insert(path.clone(), row.file.to_owned()).is_some() {
            return Err(format!("duplicate Body IR source provenance for module {path}"));
        }
    }
    let UnitContinuation {
        mut source_context,
        mut template_constructors,
        mut owners,
        sources: mut retained_sources,
        files: mut retained_files,
        entry_owner,
        entry_file,
        mut context,
        mut template_references,
        mut constructors,
        mut program,
        mut unions,
        nominal_instances: mut retained_nominals,
        intrinsic_layouts: mut intrinsics,
        models,
        enums,
        functions,
        externals,
        instances,
        emitted_frames,
        mut statics,
        instance_products,
    } = continuation;
    let mut inventories = Inventories {
        models,
        enums,
        functions,
        externals,
        instances,
    };
    let previous_nominal_count = context.nominal_declarations.len();
    let previous_model_count = inventories.models.len();
    let previous_static_count = context.static_declarations.len();
    refresh_entry_context(Arc::make_mut(&mut context), modules[entry].module);
    let additions = continuation_inputs(&source_context, &context, &owners, modules, entry)?;
    if !additions.modules.is_empty() {
        let addition_source_files = source_file_index(&additions.modules, &additions.files)?;
        let added = plan_contexts_parallel(
            additions.modules.clone(),
            &additions.sources,
            &additions.files,
            additions.entry as i64,
            false,
            &addition_source_files,
            package_versions,
            &context,
            &template_constructors,
        )?;
        extend_continuation_context(
            Arc::make_mut(&mut source_context),
            Arc::make_mut(&mut context),
            Arc::make_mut(&mut template_constructors),
            &mut owners,
            &mut retained_sources,
            &mut retained_files,
            &mut template_references,
            &mut retained_nominals,
            &additions,
            &added.source_context,
            &added.context,
            &added.template_references,
            &added.nominal_instances,
        )?;
    }
    extend_constructor_authority(
        Arc::make_mut(&mut template_constructors),
        &source_files,
        package_versions,
    );
    extend_constructor_index(Arc::make_mut(&mut constructors), &context.nominal_declarations);
    extend_generic_constructor_arities(Arc::make_mut(&mut constructors), &template_constructors);
    extend_constructor_authority(Arc::make_mut(&mut constructors), &source_files, package_versions);
    extend_unions(&mut unions, &context);
    intrinsics = intrinsic_declarations(&constructors, &context, &entry_file, intrinsics, "continuation")?;
    resume_plan_roots(
        &mut program,
        constructors.clone(),
        &context,
        &entry_file,
        &mut intrinsics,
        &retained_nominals,
        kind,
        &unions,
        previous_nominal_count,
        previous_model_count,
        &mut inventories,
    )?;
    let resolved_statics = lower_computed_statics(
        &context,
        &mut program,
        &unions,
        &retained_sources,
        &retained_files,
        &owners,
        &mut intrinsics,
        &mut inventories,
        previous_static_count,
    )?;
    let emitted_frames = rebase_emitted_frames(&context, &emitted_frames);
    statics.extend(resolved_statics);
    let requirements = selected_runtime_requirements(&context, &inventories.externals, &inventories.functions)?;
    if !requirements.is_empty() {
        return Ok(UnitPlanning::NeedsRuntime(
            UnitContinuation {
                source_context,
                template_constructors,
                owners,
                sources: retained_sources,
                files: retained_files,
                entry_owner,
                entry_file,
                context,
                template_references,
                constructors,
                program,
                unions,
                nominal_instances: retained_nominals,
                intrinsic_layouts: intrinsics,
                models: inventories.models,
                enums: inventories.enums,
                functions: inventories.functions,
                externals: inventories.externals,
                instances: inventories.instances,
                emitted_frames,
                statics,
                instance_products,
            },
            requirements,
        ));
    }
    finish_unit_plan(
        Finishing {
            template_constructors,
            constructors,
            source_context,
            template_references,
            context,
            program,
            owners,
            sources: retained_sources,
            files: retained_files,
            entry_owner,
            entry_file,
            intrinsics,
            inventories,
            emitted_frames,
            statics,
            unions,
            retained_nominals,
            products: instance_products,
        },
        kind,
        tests,
        linked_static_exports,
        published,
        trace,
    )
}
