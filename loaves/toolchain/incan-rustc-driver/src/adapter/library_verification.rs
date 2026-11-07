//! Source-backed declarations, native bridges and statics, and library layout, enum and argument verification.

use super::*;

/// Append source-backed carrier and nominal declarations in their established order. The shared source cache maps
/// each declaration span before construction; typed failures return to the callback's existing fatal boundary.
pub(super) fn add_source_backed_declarations(
    plan: &Plan,
    sources: &Sources<'_>,
    generator: &ast::attr::AttrIdGenerator,
    krate: &mut ast::Crate,
) -> Result<(), PlanError> {
    let span = sources.span(&plan.span)?;
    krate.items.extend(crate::cloneable_closures::items(plan, span)?);
    krate.items.extend(crate::formatting::items(plan, span));
    krate.items.extend(crate::zip_carriers::items(plan, span)?);
    for output in crate::futures::outputs(plan) {
        krate.items.push(crate::futures::alias(&output, krate.spans.inner_span));
        krate
            .items
            .push(crate::futures::borrowed_alias(&output, krate.spans.inner_span));
    }
    for value in &plan.enums {
        let span = sources.span(&value.span)?;
        krate.items.extend(enum_items(value, generator, span));
    }
    for value in &plan.generic_enums {
        let span = sources.span(&value.span)?;
        krate.items.extend(crate::generic_enums::items(value, generator, span));
    }
    for model in &plan.generic_models {
        let span = sources.span(&model.span)?;
        krate
            .items
            .extend(crate::generic_layouts::items(model, generator, span));
    }
    for model in &plan.models {
        let span = sources.span(&model.span)?;
        krate.items.extend(declarations::model_items(model, generator, span));
        if !model.checked_deserialize.is_empty() {
            krate.items.push(crate::checked_json::forwarding_impl(model, span));
        }
    }
    krate
        .items
        .extend(crate::foreign::items(plan, generator, krate.spans.inner_span)?);
    Ok(())
}

/// Keep the declaration order of compiler-owned adapters while propagating their typed preflight refusals.
pub(super) fn native_bridge_items(
    plan: &Plan,
    source_package: &str,
    span: rustc_span::Span,
) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut items = crate::shared_callables::items(plan, source_package, span);
    items.extend(crate::panic_hooks::items(plan, span)?);
    items.extend(crate::field_metadata::items(plan, span)?);
    items.extend(crate::comparison_bridges::items(plan, span)?);
    items.extend(crate::fallible_bridges::items(plan, source_package, span)?);
    items.extend(crate::fallible_bridges::ordinary_items(plan, source_package, span)?);
    items.extend(crate::callable_bridges::items(plan, source_package, span)?);
    items.extend(crate::native_trait_bridges::items(plan, span)?);
    Ok(items)
}

/// Keep computed storage and its accessors in declaration order; rustc retains their ordinary MIR providers.
pub(super) fn native_static_items(plan: &Plan, sources: &Sources<'_>) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut items = Vec::new();
    for cell in &plan.statics {
        let span = sources.span(&cell.span)?;
        items.extend(crate::statics::items(cell, span));
        items.extend(crate::statics::method_items(plan, cell, span)?);
    }
    Ok(items)
}

/// Reject synthesized nominal clones without a native Clone implementation before rustc constructs their MIR.
pub(super) fn verify_planned_clone_bounds(tcx: TyCtxt<'_>, plan: &Plan) -> Result<(), PlanError> {
    for block in plan.functions.iter().flat_map(|function| &function.blocks) {
        let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
            continue;
        };
        if matches!(callee.kind, CalleeKind::CloneEnum(..) | CalleeKind::CloneModel(..)) {
            let (definition, arguments) = crate::terminators::call_instance(tcx, callee)?;
            callees::verify_bounds(tcx, definition, arguments)?;
        }
    }
    Ok(())
}

/// Identify opaque return ABIs before normalization can reveal their implementation layout. Ordinary associated
/// projections remain eligible for concrete normalization; an implicit return-position `impl Trait` associated type
/// keeps the opaque boundary its caller must satisfy before constructing MIR.
pub(super) fn has_opaque_return_abi(tcx: TyCtxt<'_>, output: rustc_middle::ty::Ty<'_>) -> bool {
    match output.kind() {
        rustc_middle::ty::Alias(_, alias) => match alias.kind {
            rustc_middle::ty::Opaque { .. } => true,
            rustc_middle::ty::Projection { def_id } => tcx.is_impl_trait_in_trait(def_id),
            _ => false,
        },
        _ => false,
    }
}

/// Check every retained native layout against metadata. A field whose planned type is not its native type becomes
/// opaque in a refined plan, which is validated again: any read, write or construction relying on that field is then
/// refused, while the layout's other fields stay usable. No refinement is needed when every planned type matches.
pub(super) fn verify_library_layouts(tcx: TyCtxt<'_>, plan: &Plan) -> Result<Option<Plan>, PlanError> {
    let mut opaque = Vec::new();
    for (owner, model) in plan.models.iter().enumerate() {
        if !model.library_item.is_empty() {
            opaque.extend(verify_library_layout(tcx, model)?.into_iter().map(|slot| (owner, slot)));
        }
    }
    if opaque.is_empty() {
        return Ok(None);
    }
    let mut refined = plan.clone();
    for (owner, slot) in opaque {
        let model = &mut refined.models[owner];
        let index = i64::try_from(slot).map_err(|_| PlanError::Unsupported {
            function: model.library_item.clone(),
            reason: "native field slot overflow".into(),
        })?;
        model.fields[slot].ty = PlanType::Unit;
        model.unavailable_native_fields.push(index);
    }
    // The unit's entry was checked before the session began; every declaration and body contract is checked again.
    validation::validate(&refined, true)?;
    Ok(Some(refined))
}

/// Check a retained native layout against its crate's metadata at the plan's explicit type arguments, including their
/// bounds. Fields keep their names in order, and a field marked public must be public in metadata. Return the slots
/// whose planned type is not the field's instantiated native type, for the caller to make opaque. An opaque carrier
/// grants no projection; a field-bearing layout requires a native struct after alias instantiation.
pub(super) fn verify_library_layout(
    tcx: TyCtxt<'_>,
    model: &crate::plan::ModelDeclaration,
) -> Result<Vec<usize>, PlanError> {
    let invalid = |reason: &str| PlanError::Unsupported {
        function: model.library_item.clone(),
        reason: reason.into(),
    };
    let def = callees::native_type_item(tcx, &model.library_item)?;
    if tcx.def_kind(def) == rustc_hir::def::DefKind::Trait {
        types::native_trait_object(tcx, def, &model.library_type_arguments)?;
        return if model.fields.is_empty() {
            Ok(Vec::new())
        } else {
            Err(invalid("native trait object grants no field layout"))
        };
    }
    if !callees::arity_admits(tcx, def, model.library_type_arguments.len()) {
        return Err(invalid(
            "unsupported stdlib layout type argument count differs from metadata",
        ));
    }
    let arguments = callees::arguments(tcx, def, &model.library_type_arguments)?;
    callees::verify_bounds(tcx, def, arguments)?;
    if model.derives.iter().any(|derive| derive == "Copy") {
        let native = tcx.type_of(def).instantiate(tcx, arguments).skip_normalization();
        if !tcx.type_is_copy_modulo_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), native) {
            return Err(invalid("native Copy capability differs from metadata"));
        }
    }
    if model.fields.is_empty() {
        return Ok(Vec::new());
    }
    let native = tcx.type_of(def).instantiate(tcx, arguments).skip_normalization();
    let rustc_middle::ty::Adt(adt, arguments) = native.kind() else {
        return Err(invalid("native field layout requires a struct"));
    };
    if !adt.is_struct() {
        return Err(invalid("native field layout requires a struct"));
    }
    let declared = &adt.non_enum_variant().fields;
    if declared.len() != model.fields.len()
        && crate::generic_layouts::marker_type(tcx, *adt, arguments, model.fields.len()).is_none()
    {
        return Err(invalid("stdlib layout field count differs from metadata"));
    }
    let mut mismatched = Vec::new();
    for (slot, (field, planned)) in declared.iter().zip(&model.fields).enumerate() {
        if model.field_public.get(slot) == Some(&true) && !field.vis.is_public() {
            return Err(invalid("public native field is private in metadata"));
        }
        if field.name.as_str() != planned.name {
            return Err(invalid(&format!(
                "stdlib layout field `{}` at slot {slot} differs from metadata: native {}",
                planned.name, field.name,
            )));
        }
        let opaque = i64::try_from(slot).is_ok_and(|slot| model.unavailable_native_fields.contains(&slot));
        if !opaque && field.ty(tcx, arguments).skip_normalization() != crate::types::native_type(tcx, &planned.ty)? {
            mismatched.push(slot);
        }
    }
    Ok(mismatched)
}

/// Check the concrete SDK enum's argument count, bounds, ordered variants and instantiated payload types against
/// native metadata. Variant indices select rustc's own constructors and discriminants, including explicit tags.
/// Generic templates never authorize storage; retained Copy and Clone capabilities must hold for the concrete type.
pub(super) fn verify_library_enum(tcx: TyCtxt<'_>, value: &crate::plan::EnumDeclaration) -> Result<(), PlanError> {
    let invalid = |reason: &str| PlanError::Invalid {
        function: value.library_item.clone(),
        reason: reason.into(),
    };
    let def = callees::library_item(tcx, &value.library_item, rustc_hir::def::DefKind::Enum)?;
    if !callees::arity_admits(tcx, def, value.library_type_arguments.len()) {
        return Err(invalid(
            "unsupported stdlib enum type argument count differs from metadata",
        ));
    }
    let owner = tcx.adt_def(def);
    let declared = owner.variants();
    if declared.len() != value.variants.len() {
        return Err(invalid("unsupported stdlib enum variant count differs from metadata"));
    }
    let arguments = callees::arguments(tcx, def, &value.library_type_arguments)?;
    callees::verify_bounds(tcx, def, arguments)?;
    if value
        .derives
        .iter()
        .any(|derive| derives::from_str(derive) == Some(DeriveId::Copy))
    {
        let native = rustc_middle::ty::Ty::new_adt(tcx, owner, arguments);
        if !tcx.type_is_copy_modulo_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), native) {
            return Err(invalid("native enum Copy capability differs from metadata"));
        }
    }
    if value
        .derives
        .iter()
        .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
    {
        let owner = rustc_middle::ty::Ty::new_adt(tcx, tcx.adt_def(def), arguments);
        callees::verify_bounds(tcx, callees::clone_method(tcx)?, tcx.mk_args(&[owner.into()]))?;
    }
    for (variant, planned) in declared.iter().zip(&value.variants) {
        if variant.name.as_str() != planned.name || variant.fields.len() != planned.fields.len() {
            return Err(invalid("unsupported stdlib enum variant differs from metadata"));
        }
        for (field, ty) in variant.fields.iter().zip(&planned.fields) {
            if field.ty(tcx, arguments).skip_normalization() != crate::types::native_type(tcx, ty)? {
                return Err(invalid("unsupported stdlib enum payload differs from metadata"));
            }
        }
    }
    Ok(())
}

/// One planned enum as it is injected: its declaration with derive attributes, then the impls legacy writes by hand.
///
/// A union prints the member it holds through its own `Debug`; a value enum's serde form is its raw values, so its
/// serde derives become impls beside it instead of attributes, while any other enum derives serde as legacy does.
pub(super) fn enum_items(
    value: &crate::plan::EnumDeclaration,
    generator: &ast::attr::AttrIdGenerator,
    span: rustc_span::Span,
) -> Vec<Box<ast::Item>> {
    let value_enum = !value.raw_kind.is_empty();
    let mut item = declarations::enum_declaration(value, span);
    if value.carrier == Carrier::Declared && value.library_item.is_empty() {
        for derive in value
            .derives
            .iter()
            .filter(|derive| !(value_enum && derive.starts_with("serde::")))
        {
            item.attrs.push(declarations::derive_attribute(generator, derive, span));
        }
    }
    let mut items = vec![item];
    if value.member_debug {
        items.push(union_debug::member_debug_impl(value, span));
    }
    if value_enum && value.derives.iter().any(|derive| derive == "serde::Serialize") {
        items.push(value_enum_serde::serialize_impl(value, span));
    }
    if value_enum && value.derives.iter().any(|derive| derive == "serde::Deserialize") {
        items.push(value_enum_serde::deserialize_impl(value, span));
    }
    items
}

/// Give each generic external declared without type arguments its inferred ones, and its calls the matching
/// instantiated callee, so signature verification and MIR construction see one explicit instantiation.
pub(super) fn instantiate_generic_externals(tcx: TyCtxt<'_>, plan: &Plan) -> Result<Option<Plan>, PlanError> {
    let mut inferred = std::collections::BTreeMap::new();
    for external in &plan.externals {
        if !external.type_arguments.is_empty() {
            continue;
        }
        let def = callees::external(tcx, &external.path)?;
        if callees::arity_admits(tcx, def, 0) {
            continue;
        }
        inferred.insert(external.path.clone(), callees::inferred_arguments(tcx, def, external)?);
    }
    if inferred.is_empty() {
        return Ok(None);
    }
    let mut instantiated = plan.clone();
    for external in &mut instantiated.externals {
        if external.type_arguments.is_empty()
            && let Some(arguments) = inferred.get(&external.path)
        {
            external.type_arguments.clone_from(arguments);
        }
    }
    for block in instantiated
        .functions
        .iter_mut()
        .flat_map(|function| function.blocks.iter_mut())
    {
        let TerminatorKind::Call(callee, ..) = &mut block.terminator.kind else {
            continue;
        };
        let CalleeKind::External(path) = &callee.kind else {
            continue;
        };
        let Some(types) = inferred.get(path) else {
            continue;
        };
        callee.kind = CalleeKind::Instantiated(path.clone(), types.clone());
    }
    Ok(Some(instantiated))
}

/// Check every foreign alias's arity and parameter kinds before any native type query can expand an alias.
/// Bounds follow argument dependency order, so an invalid child cannot surface as a Rust alias diagnostic first.
pub(super) fn verify_library_argument_profiles(tcx: TyCtxt<'_>, plan: &Plan) -> Result<(), PlanError> {
    for model in plan.models.iter().filter(|model| !model.library_item.is_empty()) {
        let def = callees::native_type_item(tcx, &model.library_item)?;
        if tcx.def_kind(def) == rustc_hir::def::DefKind::Trait {
            types::native_trait_object(tcx, def, &model.library_type_arguments)?;
            continue;
        }
        let generics = tcx.generics_of(def);
        let reason = if !callees::arity_admits(tcx, def, model.library_type_arguments.len()) {
            Some("stdlib layout type argument count differs from metadata")
        } else if generics.parent_count != 0 {
            Some("native layout retains generics from an enclosing declaration")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PlanError::Unsupported {
                function: model.library_item.clone(),
                reason: reason.into(),
            });
        }
    }
    for index in validation::library_model_order(plan)? {
        let model = &plan.models[index];
        if model.library_item.is_empty() {
            continue;
        }
        let def = callees::native_type_item(tcx, &model.library_item)?;
        if tcx.def_kind(def) == rustc_hir::def::DefKind::Trait {
            continue;
        }
        let arguments = callees::arguments(tcx, def, &model.library_type_arguments)?;
        callees::verify_bounds(tcx, def, arguments)?;
    }
    Ok(())
}
